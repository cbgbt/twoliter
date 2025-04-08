use super::merge_toml::Merge;
use super::{snapshot::snapshot_from_image, AmiArgs};
use amispec::ebs::EbsBlockDeviceKind;
use amispec::{TemplateOf, TemplatedAmiSpec};
use aws_sdk_ebs::Client as EbsClient;
use aws_sdk_ec2::types::Filter;
use aws_sdk_ec2::{config::Region, Client as Ec2Client};
use buildsys::manifest::{self, ImageFeature};
use coldsnap::{SnapshotUploader, SnapshotWaiter};
use log::{debug, info, warn};
use serde::Serialize;
use snafu::{ensure, OptionExt, ResultExt};
use std::collections::HashMap;

const ROOT_DEVICE_NAME: &str = "/dev/xvda";
const DATA_DEVICE_NAME: &str = "/dev/xvdb";
const VIRT_TYPE: &str = "hvm";

lazy_static::lazy_static! {
    /// The default amispec template.
    ///
    /// pubsys modifies this structure based on attributes of the variant being registered:
    /// * The data block device mapping is removed if no data device is present
    /// * If the variant has Secure Boot enabled,
    ///     * The boot-mode is set to uefi-preferred
    ///     * uefi-data is added to the template
    static ref DEFAULT_AMISPEC_TEMPLATE: toml::Table = toml::toml! {
        name = "{{ ami.unique_name }}"
        description = "{{ ami.description }}"
        architecture = "{{ ami.arch }}"
        root-device-name = "/dev/xvda"
        sriov-net-support = "simple"
        virtualization-type = VIRT_TYPE
        ena-support = true

        [block-device-mappings."/dev/xvda".ebs]
        volume-type = "gp2"
        volume-size = "{{ block_devices.root.volume_size }}"
        snapshot-id = "{{ block_devices.root.snapshot_id }}"
        delete-on-termination = true

        [block-device-mappings."/dev/xvdb".ebs]
        volume-type = "gp2"
        volume-size = "{{ block_devices.data.volume_size }}"
        snapshot-id = "{{ block_devices.data.snapshot_id }}"
        delete-on-termination = true
    };
}

// =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
// Define the structure of the template render context for amispec templates

#[derive(Debug, Clone, Serialize)]
struct AmispecRenderContext {
    ami: AmiRenderContext,
    block_devices: HashMap<String, BlockDeviceRenderContext>,
}

#[derive(Debug, Clone, Serialize)]
struct AmiRenderContext {
    unique_name: String,
    arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BlockDeviceRenderContext {
    device_name: String,
    snapshot_id: String,
    volume_size: u64,
}

#[derive(Debug)]
pub(crate) struct RegisteredIds {
    pub(crate) image_id: String,
    pub(crate) snapshot_ids: Vec<String>,
}

/// Helper for `register_image`.  Inserts registered snapshot IDs into `cleanup_snapshot_ids` so
/// they can be cleaned up on failure if desired.
async fn _register_image(
    ami_args: &AmiArgs,
    region: &Region,
    ebs_client: EbsClient,
    ec2_client: &Ec2Client,
    cleanup_snapshot_ids: &mut Vec<String>,
) -> Result<RegisteredIds> {
    let variant_manifest = manifest::ManifestInfo::new(&ami_args.variant_manifest).context(
        error::LoadVariantManifestSnafu {
            path: &ami_args.variant_manifest,
        },
    )?;

    let image_layout = variant_manifest
        .image_layout()
        .context(error::MissingImageLayoutSnafu {
            path: &ami_args.variant_manifest,
        })?;

    debug!("Uploading images into EBS snapshots in {}", region);
    let uploader = SnapshotUploader::new(ebs_client);
    let os_snapshot =
        snapshot_from_image(&ami_args.os_image, &uploader, None, ami_args.no_progress)
            .await
            .context(error::SnapshotSnafu {
                path: &ami_args.os_image,
                region: region.as_ref(),
            })?;
    cleanup_snapshot_ids.push(os_snapshot.clone());

    let mut data_snapshot = None;
    if let Some(data_image) = &ami_args.data_image {
        let snapshot = snapshot_from_image(data_image, &uploader, None, ami_args.no_progress)
            .await
            .context(error::SnapshotSnafu {
                path: &ami_args.os_image,
                region: region.as_ref(),
            })?;
        cleanup_snapshot_ids.push(snapshot.clone());
        data_snapshot = Some(snapshot);
    }

    info!("Waiting for snapshots to become available in {}", region);
    let waiter = SnapshotWaiter::new(ec2_client.clone());
    waiter
        .wait(&os_snapshot, Default::default())
        .await
        .context(error::WaitSnapshotSnafu {
            snapshot_type: "root",
        })?;

    if let Some(ref data_snapshot) = data_snapshot {
        waiter
            .wait(&data_snapshot, Default::default())
            .await
            .context(error::WaitSnapshotSnafu {
                snapshot_type: "data",
            })?;
    }

    let uefi_secure_boot_enabled = variant_manifest
        .image_features()
        .iter()
        .flatten()
        .any(|f| *f == ImageFeature::UefiSecureBoot);

    let (os_volume_size, data_volume_size) = image_layout.publish_image_sizes_gib();
    let render_context = AmispecRenderContext {
        ami: AmiRenderContext {
            unique_name: ami_args.name.clone(),
            arch: ami_args.arch.clone(),
            description: ami_args.description.clone(),
        },
        block_devices: vec![
            Some((
                "root".into(),
                BlockDeviceRenderContext {
                    device_name: ROOT_DEVICE_NAME.into(),
                    snapshot_id: os_snapshot.clone(),
                    volume_size: os_volume_size as u64,
                },
            )),
            // Only add the data snapshot if we've registered one
            data_snapshot.as_ref().map(|snapshot| {
                (
                    "data".into(),
                    BlockDeviceRenderContext {
                        device_name: DATA_DEVICE_NAME.into(),
                        snapshot_id: snapshot.clone(),
                        volume_size: data_volume_size as u64,
                    },
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };

    let mut amispec_template_toml = DEFAULT_AMISPEC_TEMPLATE.clone();

    // Remove the data volume if the variant doesn't have one defined
    if data_snapshot.is_none() {
        amispec_template_toml
            .get_mut("block-device-mappings")
            .and_then(|bdms| bdms.as_table_mut())
            .and_then(|bdms| bdms.remove(DATA_DEVICE_NAME));
    }

    // Setup UEFI properties if secureboot is enabled
    if uefi_secure_boot_enabled {
        amispec_template_toml.insert("boot-mode".into(), "uefi-preferred".into());
        amispec_template_toml.insert("uefi-data".into(), ami_args.uefi_data.as_str().into());
    }

    // Merge user-defined properties into the default template
    if let Some(user_amispec_template) = &ami_args.amispec_file {
        info!("Merging user-defined amispec template for AMI properties");
        amispec_template_toml.merge(user_amispec_template);
    } else {
        info!("Using default amispec for AMI properties");
    }

    let amispec_template: TemplatedAmiSpec = amispec_template_toml
        .try_into()
        .context(error::ParseAmiSpecTemplateSnafu)?;
    let mut amispec = amispec_template
        .render(&render_context)
        .context(error::RenderAmiSpecSnafu)?;

    // Ensure the volume sizes aren't too small
    let snapshot_sizes: HashMap<String, i32> = [
        (Some(os_snapshot.clone()), os_volume_size),
        (data_snapshot.clone(), data_volume_size),
    ]
    .into_iter()
    .filter_map(|(snapshot, size)| snapshot.map(|snapshot| (snapshot, size)))
    .collect();
    if let Some(bdms) = amispec.block_device_mappings.as_mut() {
        for (_, bdm) in bdms.iter_mut() {
            if let Some(ebs_volume) = &mut bdm.ebs {
                if let Some(hinted_volume_size) = ebs_volume
                    .snapshot_id()
                    .and_then(|snapshot_id| snapshot_sizes.get(snapshot_id))
                    .copied()
                {
                    if let Some(curr_size) = ebs_volume.volume_size() {
                        if hinted_volume_size as u64 > curr_size {
                            warn!(
                                "EBS volume '{ebs_volume:?}' was configured with size \
                                    '{curr_size}', but size hints dictate that it should be \
                                    larger. Using volume size '{hinted_volume_size}'"
                            );
                            ebs_volume
                                .set_volume_size(Some(hinted_volume_size as u64))
                                .context(error::InvalidEbsVolumeSizeSnafu)?;
                        }
                    } else {
                        ebs_volume
                            .set_volume_size(Some(hinted_volume_size as u64))
                            .context(error::InvalidEbsVolumeSizeSnafu)?;
                    }
                }
            }
        }
    }

    debug!("Registering AMI with spec '{:?}'", amispec);

    let register_image_call = amispec.as_register_image_call();

    info!("Making register image call in {}", region);
    let register_response =
        register_image_call
            .send_with(ec2_client)
            .await
            .context(error::RegisterImageSnafu {
                region: region.as_ref(),
            })?;

    let image_id = register_response
        .image_id
        .context(error::MissingImageIdSnafu {
            region: region.as_ref(),
        })?;

    let mut snapshot_ids = vec![os_snapshot];
    if let Some(data_snapshot) = data_snapshot {
        snapshot_ids.push(data_snapshot);
    }

    Ok(RegisteredIds {
        image_id,
        snapshot_ids,
    })
}

/// Uploads the given images into snapshots and registers an AMI using them as its block device
/// mapping.  Deletes snapshots on failure.
pub(crate) async fn register_image(
    ami_args: &AmiArgs,
    region: &Region,
    ebs_client: EbsClient,
    ec2_client: &Ec2Client,
) -> Result<RegisteredIds> {
    info!("Registering '{}' in {}", ami_args.name, region);
    let mut cleanup_snapshot_ids = Vec::new();
    let register_result = _register_image(
        ami_args,
        region,
        ebs_client,
        ec2_client,
        &mut cleanup_snapshot_ids,
    )
    .await;

    if register_result.is_err() {
        for snapshot_id in cleanup_snapshot_ids {
            if let Err(e) = ec2_client
                .delete_snapshot()
                .set_snapshot_id(Some(snapshot_id.clone()))
                .send()
                .await
            {
                warn!(
                    "While cleaning up, failed to delete snapshot {}: {}",
                    snapshot_id, e
                );
            }
        }
    }
    register_result
}

/// Queries EC2 for the given AMI name. If found, returns Ok(Some(id)), if not returns Ok(None).
pub(crate) async fn get_ami_id<S>(
    name: S,
    arch: impl Into<String>,
    region: &Region,
    ec2_client: &Ec2Client,
) -> Result<Option<String>>
where
    S: Into<String>,
{
    let describe_response = ec2_client
        .describe_images()
        .set_owners(Some(vec!["self".to_string()]))
        .set_filters(Some(vec![
            Filter::builder()
                .set_name(Some("name".to_string()))
                .set_values(Some(vec![name.into()]))
                .build(),
            Filter::builder()
                .set_name(Some("architecture".to_string()))
                .set_values(Some(vec![arch.into()]))
                .build(),
            Filter::builder()
                .set_name(Some("image-type".to_string()))
                .set_values(Some(vec!["machine".to_string()]))
                .build(),
            Filter::builder()
                .set_name(Some("virtualization-type".to_string()))
                .set_values(Some(vec![VIRT_TYPE.to_string()]))
                .build(),
        ]))
        .send()
        .await
        .context(error::DescribeImagesSnafu {
            region: region.as_ref(),
        })?;
    if let Some(mut images) = describe_response.images {
        if images.is_empty() {
            return Ok(None);
        }
        ensure!(
            images.len() == 1,
            error::MultipleImagesSnafu {
                images: images
                    .into_iter()
                    .map(|i| i.image_id.unwrap_or_else(|| "<missing>".to_string()))
                    .collect::<Vec<_>>()
            }
        );
        let image = images.remove(0);
        // If there is an image but we couldn't find the ID of it, fail rather than returning None,
        // which would indicate no image.
        let id = image.image_id.context(error::MissingImageIdSnafu {
            region: region.as_ref(),
        })?;
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

mod error {
    use crate::aws::ami;
    use amispec::ebs::InvalidValueError;
    use aws_sdk_ec2::error::SdkError;
    use aws_sdk_ec2::operation::{
        describe_images::DescribeImagesError, register_image::RegisterImageError,
    };
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Failed to describe images in {}: {}", region, source))]
        DescribeImages {
            region: String,
            source: SdkError<DescribeImagesError>,
        },

        #[snafu(display("Attempted to configure EBS volume with invalid size: {}", source))]
        InvalidEbsVolumeSize { source: InvalidValueError },

        #[snafu(display("Failed to load variant manifest from {}: {}", path.display(), source))]
        LoadVariantManifest {
            path: PathBuf,
            source: buildsys::manifest::Error,
        },

        #[snafu(display("Could not find image layout for {}", path.display()))]
        MissingImageLayout { path: PathBuf },

        #[snafu(display("Image response in {} did not include image ID", region))]
        MissingImageId { region: String },

        #[snafu(display("DescribeImages with unique filters returned multiple results: {}", images.join(", ")))]
        MultipleImages { images: Vec<String> },

        #[snafu(display("Failed to parse AMI spec template: {}", source))]
        ParseAmiSpecTemplate { source: toml::de::Error },

        #[snafu(display("Failed to register image in {}: {}", region, source))]
        RegisterImage {
            region: String,
            source: SdkError<RegisterImageError>,
        },

        #[snafu(display("Failed to render amispec for RegisterImages call: {}", source))]
        RenderAmiSpec { source: amispec::TemplatedError },

        #[snafu(display("Failed to upload snapshot from {} in {}: {}", path.display(),region, source))]
        Snapshot {
            path: PathBuf,
            region: String,
            source: ami::snapshot::Error,
        },

        #[snafu(display("{} snapshot did not become available: {}", snapshot_type, source))]
        WaitSnapshot {
            snapshot_type: String,
            source: coldsnap::WaitError,
        },
    }
}
pub(crate) use error::Error;
type Result<T> = std::result::Result<T, error::Error>;
