extern crate core;

pub mod configuration;
pub mod data;
pub mod filter;
pub mod kuadrant;
pub mod metrics;
#[allow(unused_imports)]
pub(crate) mod proto;
pub mod services;
pub mod tracing;

pub(crate) const WASM_SHIM_NAME: &str = env!("CARGO_PKG_NAME");
pub(crate) const WASM_SHIM_VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const WASM_SHIM_PROFILE: &str = env!("WASM_SHIM_PROFILE");
pub(crate) const WASM_SHIM_GIT_HASH: &str = env!("WASM_SHIM_GIT_HASH");
