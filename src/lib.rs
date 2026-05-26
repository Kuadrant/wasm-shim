extern crate core;

mod configuration;
mod data;
mod filter;
mod kuadrant;
pub mod metrics;
#[allow(unused_imports)]
mod proto;
mod services;
pub mod tracing;

pub(crate) const WASM_SHIM_NAME: &str = env!("CARGO_PKG_NAME");
pub(crate) const WASM_SHIM_VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const WASM_SHIM_PROFILE: &str = env!("WASM_SHIM_PROFILE");
pub(crate) const WASM_SHIM_FEATURES: &str = env!("WASM_SHIM_FEATURES");
pub(crate) const WASM_SHIM_GIT_HASH: &str = env!("WASM_SHIM_GIT_HASH");

#[cfg_attr(
    all(target_arch = "wasm32", target_os = "wasi"),
    export_name = "_start"
)]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "wasi")), allow(dead_code))]
// This is a C interface, so make it explicit in the fn signature (and avoid mangling)
extern "C" fn start() {
    use filter::FilterRoot;
    use log::info;
    use proxy_wasm::traits::RootContext;
    use proxy_wasm::types::LogLevel;

    proxy_wasm::set_log_level(LogLevel::Trace);

    std::panic::set_hook(Box::new(|panic_info| {
        let _ = proxy_wasm::hostcalls::log(LogLevel::Critical, &panic_info.to_string());
    }));
    proxy_wasm::set_root_context(|context_id| -> Box<dyn RootContext> {
        info!("#{} set_root_context", context_id);
        Box::new(FilterRoot::new(context_id))
    });
}
