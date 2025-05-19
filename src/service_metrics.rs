use log::warn;
use proxy_wasm::types::MetricType;
use std::fmt::Debug;

#[derive(Default, Debug)]
pub struct ServiceMetrics {
    ok_metric_id: u32,
    error_metric_id: u32,
    rejected_metric_id: u32,
    failure_mode_allowed_metric_id: u32,
}

impl ServiceMetrics {
    pub fn new(
        ok_metric_id: u32,
        error_metric_id: u32,
        rejected_metric_id: u32,
        failure_mode_allowed_metric_id: u32,
    ) -> Self {
        Self {
            ok_metric_id,
            error_metric_id,
            rejected_metric_id,
            failure_mode_allowed_metric_id,
        }
    }

    pub fn from_host(metric_name_prefix: &str) -> Self {
        let ok_metric_id = match proxy_wasm::hostcalls::define_metric(
            MetricType::Counter,
            format!("{}.ok", metric_name_prefix).as_str(),
        ) {
            Ok(id) => id,
            Err(e) => panic!("Error: {:?}", e),
        };
        let error_metric_id = match proxy_wasm::hostcalls::define_metric(
            MetricType::Counter,
            format!("{}.error", metric_name_prefix).as_str(),
        ) {
            Ok(id) => id,
            Err(e) => panic!("Error: {:?}", e),
        };
        let over_limit_metric_id = match proxy_wasm::hostcalls::define_metric(
            MetricType::Counter,
            format!("{}.over_limit", metric_name_prefix).as_str(),
        ) {
            Ok(id) => id,
            Err(e) => panic!("Error: {:?}", e),
        };
        let failure_mode_allowed_metric_id = match proxy_wasm::hostcalls::define_metric(
            MetricType::Counter,
            format!("{}.failure_mode_allowed", metric_name_prefix).as_str(),
        ) {
            Ok(id) => id,
            Err(e) => panic!("Error: {:?}", e),
        };

        Self::new(
            ok_metric_id,
            error_metric_id,
            over_limit_metric_id,
            failure_mode_allowed_metric_id,
        )
    }

    #[cfg(test)]
    fn increment_metric(metric_id: u32, offset: i64) {
        tests::increment_metric(metric_id, offset);
    }

    #[cfg(not(test))]
    fn increment_metric(metric_id: u32, offset: i64) {
        if let Err(e) = proxy_wasm::hostcalls::increment_metric(metric_id, offset) {
            warn!("proxy_wasm::hostcalls::increment_metric metric {metric_id}, offset {offset} , error: {e:?}");
        }
    }

    pub fn report_error(&self) {
        Self::increment_metric(self.error_metric_id, 1);
    }

    pub fn report_allowed_on_failure(&self) {
        Self::increment_metric(self.failure_mode_allowed_metric_id, 1);
    }

    pub fn report_ok(&self) {
        Self::increment_metric(self.ok_metric_id, 1);
    }

    pub fn report_rejected(&self) {
        Self::increment_metric(self.rejected_metric_id, 1);
    }
}

#[cfg(test)]
mod tests {
    use log::debug;
    use std::cell::Cell;

    thread_local!(
        pub static TEST_INCREMENT_METRIC_VALUE: Cell<Option<(u32, i64)>> =
            const { Cell::new(None) };
    );

    pub fn increment_metric(metric_id: u32, offset: i64) {
        debug!("increment_metric: metric_id: {metric_id}, offset: {offset}");
        match TEST_INCREMENT_METRIC_VALUE.take() {
            None => panic!(
                "unexpected call to increment metric metric_id: {metric_id} offset: {offset}"
            ),
            Some((expected_metric_id, expected_offset)) => {
                assert_eq!(expected_metric_id, metric_id);
                assert_eq!(expected_offset, offset);
            }
        }
    }
}
