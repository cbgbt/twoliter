//! This module implements serde-based (de-)serialization for EBS block devices.
//!
//! Performs client-side validation of inputs (e.g. properly checks that invalid fields aren't
//! enabled based on volume type, or that requested volume sizes are within bounds.)
use aws_sdk_ec2::types::builders::EbsBlockDeviceBuilder;
use aws_sdk_ec2::types::{EbsBlockDevice as SdkEbsBlockDevice, VolumeType as SdkVolumeType};
use bon::Builder;
use bounded_integer::BoundedU64;
use num_traits::AsPrimitive;
use serde::{Deserialize, Serialize};
use serde_templated::TemplateOf;
use serde_templated::Templated;
use snafu::{OptionExt, Snafu};
use std::fmt::Debug;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "volume-type")]
pub enum EbsBlockDevice {
    Standard(Standard),
    Io1(Io1),
    Io2(Io2),
    Gp2(Gp2),
    Gp3(Gp3),
    St1(St1),
    Sc1(Sc1),
}

impl EbsBlockDevice {
    pub(crate) fn create_sdk_ebs_block_device(&self) -> SdkEbsBlockDevice {
        let builder = EbsBlockDeviceBuilder::default();
        match self {
            EbsBlockDevice::Standard(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Standard),
            EbsBlockDevice::Io1(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Io1),
            EbsBlockDevice::Io2(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Io2),
            EbsBlockDevice::Gp2(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Gp2),
            EbsBlockDevice::Gp3(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Gp3),
            EbsBlockDevice::St1(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::St1),
            EbsBlockDevice::Sc1(device) => device
                .configure_ebs_block_device(builder)
                .volume_type(SdkVolumeType::Sc1),
        }
        .build()
    }

    pub fn as_ebs_device(&self) -> &dyn EbsBlockDeviceKind {
        match self {
            EbsBlockDevice::Standard(device) => device,
            EbsBlockDevice::Io1(device) => device,
            EbsBlockDevice::Io2(device) => device,
            EbsBlockDevice::Gp2(device) => device,
            EbsBlockDevice::Gp3(device) => device,
            EbsBlockDevice::St1(device) => device,
            EbsBlockDevice::Sc1(device) => device,
        }
    }

    pub fn as_mut_ebs_device(&mut self) -> &mut dyn EbsBlockDeviceKind {
        match self {
            EbsBlockDevice::Standard(device) => device,
            EbsBlockDevice::Io1(device) => device,
            EbsBlockDevice::Io2(device) => device,
            EbsBlockDevice::Gp2(device) => device,
            EbsBlockDevice::Gp3(device) => device,
            EbsBlockDevice::St1(device) => device,
            EbsBlockDevice::Sc1(device) => device,
        }
    }
}

/// Templated `EbsBlockDevice` type
///
/// Implemented manually because the `Templated` macro does not support enums.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "volume-type")]
pub enum TemplatedEbsBlockDevice {
    Standard(TemplatedStandard),
    Io1(TemplatedIo1),
    Io2(TemplatedIo2),
    Gp2(TemplatedGp2),
    Gp3(TemplatedGp3),
    St1(TemplatedSt1),
    Sc1(TemplatedSc1),
}

impl TemplateOf for TemplatedEbsBlockDevice {
    type Target = EbsBlockDevice;

    fn render(
        &self,
        template_context: &impl Serialize,
    ) -> Result<Self::Target, serde_templated::TemplatedError> {
        Ok(match self {
            Self::Standard(standard) => {
                EbsBlockDevice::Standard(standard.render(template_context)?)
            }
            Self::Io1(io1) => EbsBlockDevice::Io1(io1.render(template_context)?),
            Self::Io2(io2) => EbsBlockDevice::Io2(io2.render(template_context)?),
            Self::Gp2(gp2) => EbsBlockDevice::Gp2(gp2.render(template_context)?),
            Self::Gp3(gp3) => EbsBlockDevice::Gp3(gp3.render(template_context)?),
            Self::St1(st1) => EbsBlockDevice::St1(st1.render(template_context)?),
            Self::Sc1(sc1) => EbsBlockDevice::Sc1(sc1.render(template_context)?),
        })
    }
}

/// Abstracts over EBS volume types and allow fetching and setting of common keys.
pub trait EbsBlockDeviceKind {
    fn volume_type(&self) -> &str;

    fn volume_size(&self) -> Option<u64>;
    fn set_volume_size(&mut self, volume_size: Option<u64>) -> Result<(), InvalidValueError>;

    fn delete_on_termination(&self) -> Option<bool>;
    fn set_delete_on_termination(&mut self, delete_on_termination: Option<bool>);

    fn encrypted(&self) -> Option<bool>;
    fn set_encrypted(&mut self, encrypted: Option<bool>);

    fn kms_key_id(&self) -> Option<&str>;
    fn set_kms_key_id(&mut self, kms_key_id: Option<String>);

    fn outpost_arn(&self) -> Option<&str>;
    fn set_outpost_arn(&mut self, outpost_arn: Option<String>);

    fn snapshot_id(&self) -> Option<&str>;
    fn set_snapshot_id(&mut self, snapshot_id: Option<String>);
}

/// Creates a BoundedU64 from a u64, returning [`InvalidValueError`] if the input is out of bounds.
///
/// This is needed because [`TryFrom`] is not implemented by [`bounded_integer`].
fn bounded_from_u64<const MIN: u64, const MAX: u64>(
    value: u64,
    volume_type: &str,
    field: &str,
) -> Result<BoundedU64<MIN, MAX>, InvalidValueError> {
    BoundedU64::new(value).context(invalid_value_error::InvalidValueSnafu {
        min: MIN,
        max: MAX,
        field,
        volume_type,
        value,
    })
}

#[derive(Debug, Snafu)]
#[snafu(module)]
#[snafu(display(
    "Invalid value {value}' not within bounds [{min}, {max}] \
        for field '{field}' of volume-type '{volume_type}'"
))]
pub struct InvalidValueError {
    value: u64,
    min: u64,
    max: u64,
    field: String,
    volume_type: String,
}

/// Macro to help create an EBS "volume-type" that performs client-side validation of configuration
/// values.
///
/// This macro is used to share common EBS configuration options between types because
/// `serde(flatten)` is not supported to be used with `serde(deny_unknown_keys)`.
macro_rules! impl_ebs_volume_type {
    (
        $name:ident ($awsname:expr) {
            template: $templated_ty:ty,
            volume_size: $size_ty:ident,
        }

        build(&$self:ident, $builder:ident) {
            $($impl_body:tt)*
        }
    ) => {
        impl $name {
            fn configure_ebs_block_device(
                &$self,
                mut $builder: EbsBlockDeviceBuilder,
            ) -> EbsBlockDeviceBuilder {
                $builder = $builder
                    .set_delete_on_termination($self.delete_on_termination)
                    .set_encrypted($self.encrypted)
                    .set_kms_key_id($self.kms_key_id.clone())
                    .set_outpost_arn($self.outpost_arn.clone())
                    .set_snapshot_id($self.snapshot_id.clone());
                $($impl_body)*
            }

            pub fn volume_type() -> &'static str {
                $awsname
            }
        }

        impl EbsBlockDeviceKind for $name {
            fn volume_type(&self) -> &'static str {
                Self::volume_type()
            }

            fn volume_size(&self) -> Option<u64> {
                self.volume_size.map(AsPrimitive::as_)
            }

            fn set_volume_size(&mut self, volume_size: Option<u64>) -> Result<(), InvalidValueError> {
                if let Some(volume_size) = volume_size {
                    let volume_type = Self::volume_type();
                    self.volume_size = Some(bounded_from_u64(volume_size, volume_type, "volume-size")?);
                }
                Ok(())
            }

            fn delete_on_termination(&self) -> Option<bool> {
                self.delete_on_termination
            }

            fn set_delete_on_termination(&mut self, delete_on_termination: Option<bool>) {
                self.delete_on_termination = delete_on_termination;
            }

            fn encrypted(&self) -> Option<bool> {
                self.encrypted
            }

            fn set_encrypted(&mut self, encrypted: Option<bool>) {
                self.encrypted = encrypted;
            }

            fn kms_key_id(&self) -> Option<&str> {
                self.kms_key_id.as_deref()
            }

            fn set_kms_key_id(&mut self, kms_key_id: Option<String>) {
                self.kms_key_id = kms_key_id;
            }

            fn outpost_arn(&self) -> Option<&str> {
                self.outpost_arn.as_deref()
            }

            fn set_outpost_arn(&mut self, outpost_arn: Option<String>) {
                self.outpost_arn = outpost_arn;
            }

            fn snapshot_id(&self) -> Option<&str> {
                self.snapshot_id.as_deref()
            }

            fn set_snapshot_id(&mut self, snapshot_id: Option<String>) {
                self.snapshot_id = snapshot_id;
            }
        }

        impl From<$name> for EbsBlockDevice {
            fn from(value: $name) -> Self {
                Self::$name(value)
            }
        }

        impl From<$templated_ty> for TemplatedEbsBlockDevice {
            fn from(value: $templated_ty) -> Self {
                Self::$name(value)
            }
        }
    };
}

type StandardVolumeSize = BoundedU64<1, 1_024>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated, Default)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Standard {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Standard::volume_type(), "volume-size")
    })]
    pub volume_size: Option<StandardVolumeSize>,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Standard ("standard") {
        template: TemplatedStandard,
        volume_size: StandardVolumeSize,
    }

    build(&self, builder) {
        builder.set_volume_size(self.volume_size.map(AsPrimitive::as_))
    }
}

type Io1VolumeSize = BoundedU64<4, 16_384>;
type Io1Iops = BoundedU64<100, 64_000>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Io1 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Io1::volume_type(), "volume-size")
    })]
    pub volume_size: Option<Io1VolumeSize>,
    pub iops: Io1Iops,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Io1 ("io1") {
        template: TemplatedIo1,
        volume_size: Io1VolumeSize,
    }

    build(&self, builder) {
        builder
            .set_volume_size(self.volume_size.map(AsPrimitive::as_))
            .iops(self.iops.as_())
    }
}

type Io2VolumeSize = BoundedU64<4, 65_536>;
type Io2Iops = BoundedU64<100, 256_000>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Io2 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Io2::volume_type(), "volume-size")
    })]
    pub volume_size: Option<Io2VolumeSize>,
    pub iops: Io2Iops,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Io2 ("io2") {
        template: TemplatedIo2,
        volume_size: Io2VolumeSize,
    }

    build(&self, builder) {
        builder
            .set_volume_size(self.volume_size.map(AsPrimitive::as_))
            .iops(self.iops.as_())
    }
}

type Gp2VolumeSize = BoundedU64<1, 16_384>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated, Default)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Gp2 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Gp2::volume_type(), "volume-size")
    })]
    pub volume_size: Option<Gp2VolumeSize>,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Gp2 ("gp2") {
        template: TemplatedGp2,
        volume_size: Gp2VolumeSize,
    }

    build(&self, builder) {
        builder.set_volume_size(self.volume_size.map(AsPrimitive::as_))
    }
}

type Sc1VolumeSize = BoundedU64<125, 16_384>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated, Default)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Sc1 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Sc1::volume_type(), "volume-size")
    })]
    pub volume_size: Option<Sc1VolumeSize>,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Sc1 ("sc1") {
        template: TemplatedSc1,
        volume_size: Sc1VolumeSize,
    }

    build(&self, builder) {
        builder.set_volume_size(self.volume_size.map(AsPrimitive::as_))
    }
}

type St1VolumeSize = BoundedU64<125, 16_384>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated, Default)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct St1 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, St1::volume_type(), "volume-size")
    })]
    volume_size: Option<St1VolumeSize>,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    St1 ("st1") {
        template: TemplatedSt1,
        volume_size: St1VolumeSize,
    }

    build(&self, builder) {
        builder.set_volume_size(self.volume_size.map(AsPrimitive::as_))
    }
}

type Gp3Iops = BoundedU64<3_000, 16_000>;
type Gp3Throughput = BoundedU64<125, 1_000>;
type Gp3VolumeSize = BoundedU64<1, 16_384>;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Builder, Templated, Default)]
#[templated(
    derive(Debug, Clone, Eq, PartialEq, Builder),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct Gp3 {
    #[builder(with = |size: u64| -> Result<_, InvalidValueError> {
        bounded_from_u64(size, Gp3::volume_type(), "volume-size")
    })]
    pub volume_size: Option<Gp3VolumeSize>,
    pub iops: Option<Gp3Iops>,
    pub throughput: Option<Gp3Throughput>,

    // Common EBS attributes
    pub delete_on_termination: Option<bool>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub outpost_arn: Option<String>,
    pub snapshot_id: Option<String>,
}

impl_ebs_volume_type! {
    Gp3 ("gp3") {
        template: TemplatedGp3,
        volume_size: Gp3VolumeSize,
    }

    build(&self, builder) {
        builder
            .set_iops(self.iops.map(AsPrimitive::as_))
            .set_throughput(self.throughput.map(AsPrimitive::as_))
            .set_volume_size(self.volume_size.map(AsPrimitive::as_))
    }
}

impl EbsBlockDeviceKind for EbsBlockDevice {
    fn volume_type(&self) -> &str {
        self.as_ebs_device().volume_type()
    }

    fn volume_size(&self) -> Option<u64> {
        self.as_ebs_device().volume_size()
    }

    fn delete_on_termination(&self) -> Option<bool> {
        self.as_ebs_device().delete_on_termination()
    }

    fn encrypted(&self) -> Option<bool> {
        self.as_ebs_device().encrypted()
    }

    fn kms_key_id(&self) -> Option<&str> {
        self.as_ebs_device().kms_key_id()
    }

    fn outpost_arn(&self) -> Option<&str> {
        self.as_ebs_device().outpost_arn()
    }

    fn snapshot_id(&self) -> Option<&str> {
        self.as_ebs_device().snapshot_id()
    }

    fn set_volume_size(&mut self, volume_size: Option<u64>) -> Result<(), InvalidValueError> {
        self.as_mut_ebs_device().set_volume_size(volume_size)
    }

    fn set_delete_on_termination(&mut self, delete_on_termination: Option<bool>) {
        self.as_mut_ebs_device()
            .set_delete_on_termination(delete_on_termination);
    }

    fn set_encrypted(&mut self, encrypted: Option<bool>) {
        self.as_mut_ebs_device().set_encrypted(encrypted);
    }

    fn set_kms_key_id(&mut self, kms_key_id: Option<String>) {
        self.as_mut_ebs_device().set_kms_key_id(kms_key_id);
    }

    fn set_outpost_arn(&mut self, outpost_arn: Option<String>) {
        self.as_mut_ebs_device().set_outpost_arn(outpost_arn);
    }

    fn set_snapshot_id(&mut self, snapshot_id: Option<String>) {
        self.as_mut_ebs_device().set_snapshot_id(snapshot_id);
    }
}
