mod kuadrant_filter;
mod root_context;

pub use root_context::FilterRoot;

#[cfg(test)]
mod tests {
    use crate::configuration::PluginConfiguration;
    use crate::filter::kuadrant_filter::KuadrantFilter;
    use crate::kuadrant::PipelineFactory;
    use std::rc::Rc;

    fn create_filter(cfg: &str) -> KuadrantFilter {
        let factory: PipelineFactory = serde_json::from_str::<PluginConfiguration>(cfg)
            .expect("Must parse!")
            .try_into()
            .expect("Must convert!");

        KuadrantFilter::new(1, Rc::new(factory))
    }

    mod auth {
        use std::sync::Arc;
        use super::create_filter;
        use proxy_wasm::traits::HttpContext;
        use proxy_wasm::types::Action;
        use proxy_wasm_test_framework::types::LogLevel;
        use crate::kuadrant::MockWasmHost;

        const CONFIG: &str = r#"{
    "services": {
        "authorino": {
            "type": "auth",
            "endpoint": "authorino-cluster",
            "failureMode": "deny",
            "timeout": "5s"
        }
    },
    "actionSets": [
    {
        "name": "some-name",
        "routeRuleConditions": {
            "hostnames": ["*.toystore.com", "example.com"],
            "predicates" : [
                "request.url_path.startsWith('/admin/toy')",
                "request.host == 'cars.toystore.com'",
                "request.method == 'POST'"
            ]
        },
        "actions": [
        {
            "service": "authorino",
            "scope": "authconfig-A"
        }]
    }]
}"#;
        #[test]
        fn it_auths() {
            let mut filter = create_filter(CONFIG);

            let mock_host = Arc::new(MockWasmHost::new()
                .with_property("request.host".into(), "cars.toystore.com".as_bytes().to_vec())
                .with_property(
                    "request.url_path".into(),
                    "/admin/toy".as_bytes().to_vec(),
                )
                .with_property(
                    "request.path".into(),
                    "/admin/toy".as_bytes().to_vec(),
                )
                .with_map("request.headers".into(), Vec::default())
                .with_property("request.method".into(), "POST".as_bytes().to_vec())
                .with_property("request.protocol".into(), "HTTP/1.1".as_bytes().to_vec())
                .with_property("request.scheme".into(), "http".as_bytes().to_vec())
                .with_property("request.time".into(), vec![240, 158, 152, 213, 254, 179, 5, 24])
                .with_property("destination.address".into(), "127.0.0.1:45000".as_bytes().to_vec())
                .with_property("destination.port".into(), vec![64, 31, 0, 0, 0, 0, 0, 0])
                .with_property("source.address".into(), "127.0.0.1:45000".as_bytes().to_vec())
                .with_property("source.port".into(), vec![200, 175, 0, 0, 0, 0, 0, 0])
            );

            crate::kuadrant::MOCK.set(Some(mock_host.clone()));

            assert_eq!(filter.on_http_request_headers(0, false), Action::Pause);
        }
    }
}
