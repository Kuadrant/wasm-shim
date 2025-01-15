pub(crate) mod kuadrant_filter;
pub(crate) mod operations;
mod root_context;

#[cfg_attr(
    all(
        target_arch = "wasm32",
        target_vendor = "unknown",
        target_os = "unknown"
    ),
    export_name = "_start"
)]
#[cfg_attr(
    not(all(
        target_arch = "wasm32",
        target_vendor = "unknown",
        target_os = "unknown"
    )),
    allow(dead_code)
)]
// This is a C interface, so make it explicit in the fn signature (and avoid mangling)
extern "C" fn start() {
    use log::info;
    use proxy_wasm::traits::RootContext;
    use proxy_wasm::types::LogLevel;
    use root_context::FilterRoot;

    proxy_wasm::set_log_level(LogLevel::Trace);
    std::panic::set_hook(Box::new(|panic_info| {
        proxy_wasm::hostcalls::log(LogLevel::Critical, &panic_info.to_string())
            .expect("failed to log panic_info");
    }));
    proxy_wasm::set_root_context(|context_id| -> Box<dyn RootContext> {
        info!("#{} set_root_context", context_id);
        Box::new(FilterRoot {
            context_id,
            action_set_index: Default::default(),
        })
    });
}
