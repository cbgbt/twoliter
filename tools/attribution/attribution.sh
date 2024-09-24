#!/bin/bash
# Generates attributions for dependencies of Twoliter
# Meant to be run from Bottlerocket's SDK container:
# https://github.com/bottlerocket-os/bottlerocket-sdk

# See the "attribution" target in the project Makefile.

set -eo pipefail

LICENSEDIR=/tmp/twoliter-attributions

# Use the toolchain installed via `Dockerfile.attribution`
export HOME="/home/attribution-creator"
source ~/.cargo/env

# Source code is mounted to /src
# rustup will automatically use the toolchain in rust-toolchain.toml
cd /src

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
echo "Clarifying crate dependency licenses..."
/usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir ${LICENSEDIR}/vendor \
    cargo --locked Cargo.toml


# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
echo "Clarifying cargo-cross & dependency licenses..."
git clone https://github.com/cross-rs/cross/ /tmp/cargo-cross
pushd /tmp/cargo-cross
git reset --hard 7b79041
popd
/usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir ${LICENSEDIR}/cross/vendor \
    cargo --locked /tmp/cargo-cross/Cargo.toml
cp /tmp/cargo-cross/LICENSE-APACHE /tmp/cargo-cross/LICENSE-MIT \
    ${LICENSEDIR}/cross/

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
echo "Clarifying cargo-dist & dependency licenses..."
git clone https://github.com/webern/cargo-dist/ /tmp/cargo-dist
pushd /tmp/cargo-dist
git reset --hard 3dcbe823
popd
/usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir ${LICENSEDIR}/cargo-dist/vendor \
    cargo --locked /tmp/cargo-dist/Cargo.toml
cp /tmp/cargo-dist/LICENSE-APACHE /tmp/cargo-dist/LICENSE-MIT \
    ${LICENSEDIR}/cargo-dist/

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
# SDK Dependencies
echo "Clarifying bottlerocket-sdk & dependency licenses..."
mkdir -p ${LICENSEDIR}/bottlerocket-sdk/
cp -r /usr/share/licenses/rust /usr/share/licenses/cargo-make \
    ${LICENSEDIR}/bottlerocket-sdk/

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
# Twoliter licenses
cp /src/COPYRIGHT /src/LICENSE-MIT /src/LICENSE-APACHE \
    ${LICENSEDIR}/

pushd $(dirname ${LICENSEDIR})
tar czf /src/twoliter-attributions.tar.gz $(basename ${LICENSEDIR})
popd
