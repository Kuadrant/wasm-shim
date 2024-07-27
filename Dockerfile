# ------------------------------------------------------------------------------
# Build Stage
# ------------------------------------------------------------------------------

FROM alpine:3.16 as wasm-shim-build

ARG GITHUB_SHA
ENV GITHUB_SHA=${GITHUB_SHA:-unknown}

ARG RUSTC_VERSION=1.80.0
RUN apk update \
    && apk upgrade \
    && apk add build-base binutils-gold openssl3-dev protoc curl \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- --no-modify-path --profile minimal --default-toolchain ${RUSTC_VERSION} \
      -c rustfmt -t wasm32-unknown-unknown -y

WORKDIR /usr/src/wasm-shim

COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

COPY src src
COPY build.rs build.rs
COPY vendor-protobufs vendor-protobufs

RUN source $HOME/.cargo/env \
    && cargo build --target=wasm32-unknown-unknown --release

# ------------------------------------------------------------------------------
# Run Stage
# ------------------------------------------------------------------------------

FROM scratch

COPY --from=wasm-shim-build /usr/src/wasm-shim/target/wasm32-unknown-unknown/release/wasm_shim.wasm /plugin.wasm
