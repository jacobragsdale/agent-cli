# The image azure-pipelines.yml runs every check and the release build in.
#
# Ubuntu 22.04, so the release binary needs glibc 2.35 at most and runs on
# 22.04 and every newer Ubuntu, WSL's included (a rust:*-bookworm image would
# link against 2.36). The toolchain is pinned so a new Rust release cannot
# turn a clippy lint into a red main; bump RUST_VERSION on purpose.
#
#   docker build -t agent-cli-ci ci
FROM ubuntu:22.04

ARG RUST_VERSION=1.99.1

# build-essential: the Oracle driver (ODPI-C) and ring compile C.
# git and python3: tests and scripts call them. binutils (objdump):
# release-build.sh's glibc check.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential pkg-config ca-certificates curl git python3 binutils xz-utils \
    && rm -rf /var/lib/apt/lists/*

# The toolchain lives outside any home directory and is readable by every
# user, because the pipeline runs the container as the agent's own user (so
# target/ and the cargo cache stay the agent's) and points CARGO_HOME at a
# cache directory it mounts.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain "$RUST_VERSION" --component clippy,rustfmt \
    && chmod -R a+rX "$RUSTUP_HOME" "$CARGO_HOME" \
    && rustc --version && cargo clippy --version && cargo fmt --version
