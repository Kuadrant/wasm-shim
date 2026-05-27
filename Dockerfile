# ------------------------------------------------------------------------------
# Build Stage
# ------------------------------------------------------------------------------

FROM mirror.gcr.io/library/alpine:3.23 AS wasm-shim-build

ARG GITHUB_SHA
ENV GITHUB_SHA=${GITHUB_SHA:-unknown}

ARG RUSTC_VERSION=1.88.0
RUN apk update \
    && apk upgrade \
    && apk add build-base binutils-gold openssl3-dev protoc curl \
    && ARCH=$(uname -m) \
    && case "${ARCH}" in \
        x86_64) RUSTUP_ARCH="x86_64-unknown-linux-musl" ;; \
        aarch64) RUSTUP_ARCH="aarch64-unknown-linux-musl" ;; \
        armv7l) RUSTUP_ARCH="armv7-unknown-linux-musleabihf" ;; \
        *) echo "Unsupported architecture: ${ARCH}" && exit 1 ;; \
       esac \
    && curl -LO "https://static.rust-lang.org/rustup/dist/${RUSTUP_ARCH}/rustup-init" \
    && curl -LO "https://static.rust-lang.org/rustup/dist/${RUSTUP_ARCH}/rustup-init.sha256" \
    && sha256sum -c rustup-init.sha256 \
    && chmod +x rustup-init \
    && ./rustup-init --no-modify-path --profile minimal --default-toolchain ${RUSTC_VERSION} \
       -c rustfmt -t wasm32-wasip1 -y \
    && rm rustup-init rustup-init.sha256

WORKDIR /usr/src/wasm-shim

COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

COPY src src
COPY build.rs build.rs
COPY vendor-protobufs vendor-protobufs

RUN source $HOME/.cargo/env \
    && cargo build --target=wasm32-wasip1 --release

# ------------------------------------------------------------------------------
# Run Stage
# ------------------------------------------------------------------------------

FROM scratch

COPY --from=wasm-shim-build /usr/src/wasm-shim/target/wasm32-wasip1/release/wasm_shim.wasm /plugin.wasm
