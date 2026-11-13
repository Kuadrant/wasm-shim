use std::error::Error;
use std::io::{Read, Write};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use flate2::write::GzEncoder;
use flate2::Compression;
use kuadrant_filter::configuration::encoding::{decode_configuration, MAX_DECODED_BYTES};
use proxy_wasm_test_framework::tester::{self, Tester};
use proxy_wasm_test_framework::types::{
    Action, BufferType, LogLevel, MapType, MetricType, ReturnType,
};
use serial_test::serial;

use util::common::{wasm_module, LOG_LEVEL};
use util::data;
use util::data::compressed_config::{GO_HEADERS, GRPC, HEADERS};

pub mod util;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn gzip(input: &[u8]) -> TestResult<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input)?;
    Ok(encoder.finish()?)
}

fn envelope(compressed: &[u8]) -> Vec<u8> {
    serde_json::json!({"encoding":"gzip+base64", "data":STANDARD.encode(compressed)})
        .to_string()
        .into_bytes()
}

fn module() -> TestResult<Tester> {
    let mut module = tester::mock(tester::MockSettings {
        wasm_path: wasm_module(),
        quiet: true,
        allow_unexpected: false,
    })?;
    module.call_start().execute_and_expect(ReturnType::None)?;
    module
        .call_proxy_on_context_create(1, 0)
        .expect_log(Some(LogLevel::Info), Some("#1 set_root_context"))
        .execute_and_expect(ReturnType::None)?;
    Ok(module)
}

fn configure(module: &mut Tester, config: &[u8]) {
    module
        .call_proxy_on_configure(1, config.len() as i32)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"));
    for (index, name) in [
        "kuadrant.configs",
        "kuadrant.hits",
        "kuadrant.misses",
        "kuadrant.allowed",
        "kuadrant.denied",
        "kuadrant.errors",
        "kuadrant.body_extraction_misses",
    ]
    .into_iter()
    .enumerate()
    {
        module
            .expect_define_metric(Some(MetricType::Counter), Some(name))
            .returning(Some(index as i32 + 1));
    }
    module
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(config));
}

fn assert_header_change(module: &mut Tester, context: i32) -> TestResult {
    module
        .call_proxy_on_context_create(context, 1)
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::None)?;
    module
        .call_proxy_on_request_headers(context, 0, false)
        .expect_get_property(Some(vec!["request", "host"]))
        .returning(Some(b"example.com"))
        .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
        .returning(Some(vec![("x-config", "before")]))
        .expect_increment_metric(Some(2), Some(1))
        .expect_set_header_map_pairs(
            Some(MapType::HttpRequestHeaders),
            Some(vec![("x-config", "decoded")]),
        )
        .execute_and_expect(ReturnType::Action(Action::Continue))?;
    Ok(())
}

#[test]
#[serial]
fn plain_and_go_compressed_configs_modify_the_same_headers() -> TestResult {
    for config in [HEADERS, GO_HEADERS] {
        let mut module = module()?;
        configure(&mut module, config);
        module
            .expect_get_log_level()
            .returning(Some(LogLevel::Info))
            .expect_log(Some(LogLevel::Info), Some("plugin config parsed"))
            .execute_and_expect(ReturnType::Bool(true))?;
        assert_header_change(&mut module, 2)?;
    }
    Ok(())
}

#[test]
#[serial]
fn plain_and_compressed_configs_handle_the_same_grpc_response() -> TestResult {
    let compressed = envelope(&gzip(GRPC)?);
    for config in [GRPC, compressed.as_slice()] {
        let mut module = module()?;
        configure(&mut module, config);
        module
            .expect_get_log_level()
            .returning(Some(LOG_LEVEL))
            .expect_grpc_call(
                Some("descriptor-service-cluster"),
                Some("kuadrant.v1.DescriptorService"),
                Some("GetServiceDescriptors"),
                None,
                None,
                Some(1000),
            )
            .returning(Ok(42))
            .expect_set_tick_period_millis(Some(2000))
            .execute_and_expect(ReturnType::Bool(true))?;
        let descriptors = data::descriptor_response::TEST_SERVICE;
        module
            .call_proxy_on_grpc_receive(1, 42, descriptors.len() as i32)
            .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
            .returning(Some(descriptors))
            .execute_and_expect(ReturnType::None)?;
        module
            .call_proxy_on_context_create(2, 1)
            .expect_get_log_level()
            .returning(Some(LOG_LEVEL))
            .execute_and_expect(ReturnType::None)?;
        module
            .call_proxy_on_request_headers(2, 0, false)
            .expect_get_property(Some(vec!["request", "host"]))
            .returning(Some(data::request::HOST))
            .expect_get_header_map_pairs(Some(MapType::HttpRequestHeaders))
            .returning(None)
            .expect_increment_metric(Some(2), Some(1))
            .expect_grpc_call(
                Some("limitador-cluster"),
                Some("test.TestService"),
                Some("TestMethod"),
                None,
                Some(&[10, 5, b'h', b'e', b'l', b'l', b'o']),
                Some(5000),
            )
            .returning(Ok(43))
            .execute_and_expect(ReturnType::Action(Action::Pause))?;
        let response = data::dynamic_response::RESULT_DENIED;
        module
            .call_proxy_on_grpc_receive(2, 43, response.len() as i32)
            .expect_get_buffer_bytes(Some(BufferType::GrpcReceiveBuffer))
            .returning(Some(response))
            .expect_increment_metric(Some(5), Some(1))
            .expect_send_local_response(
                Some(429),
                Some("Too Many Requests"),
                Some(vec![("x-ratelimit-reason", "denied")]),
                None,
            )
            .execute_and_expect(ReturnType::None)?;
    }
    Ok(())
}

#[test]
#[serial]
fn rejects_invalid_compressed_configuration() -> TestResult {
    let mut corrupt = gzip(HEADERS)?;
    let crc = corrupt.len() - 8;
    corrupt[crc] ^= 1;
    for config in [
        br#"{"encoding":"unknown","data":""}"#.to_vec(),
        br#"{"encoding":"gzip+base64","data":"invalid base64"}"#.to_vec(),
        envelope(&corrupt),
        envelope(&gzip(br#"{"encoding":"gzip+base64","data":""}"#)?),
    ] {
        let error = decode_configuration(&config)
            .err()
            .ok_or("expected invalid fixture")?;
        let message = format!("failed to load plugin config: {error}");
        let mut module = module()?;
        configure(&mut module, &config);
        module
            .expect_log(Some(LogLevel::Error), Some(&message))
            .execute_and_expect(ReturnType::Bool(false))?;
    }
    Ok(())
}

#[test]
#[serial]
fn rejects_plain_json_syntax_errors_without_compression_diagnostics() -> TestResult {
    let config = br#"{"services": {}, "actionSets": [],}"#;
    let error = serde_json::from_slice::<serde_json::Value>(config)
        .err()
        .ok_or("expected invalid JSON fixture")?;
    let message = format!("failed to load plugin config: {error}");
    let mut module = module()?;
    configure(&mut module, config);
    module
        .expect_log(Some(LogLevel::Error), Some(&message))
        .execute_and_expect(ReturnType::Bool(false))?;
    Ok(())
}

#[test]
#[ignore = "checks 32 MiB decompression in the WASM runtime"]
#[serial]
fn accepts_the_decoded_limit_and_rejects_one_byte_above() -> TestResult {
    for length in [MAX_DECODED_BYTES, MAX_DECODED_BYTES + 1] {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(HEADERS)?;
        let mut padding = std::io::repeat(b' ').take((length - HEADERS.len()) as u64);
        std::io::copy(&mut padding, &mut encoder)?;
        let config = envelope(&encoder.finish()?);
        let mut module = module()?;
        configure(&mut module, &config);
        if length == MAX_DECODED_BYTES {
            module
                .expect_get_log_level()
                .returning(Some(LOG_LEVEL))
                .execute_and_expect(ReturnType::Bool(true))?;
            assert_header_change(&mut module, 2)?;
        } else {
            let message = format!(
                "failed to load plugin config: decoded configuration exceeds {MAX_DECODED_BYTES} bytes"
            );
            module
                .expect_log(Some(LogLevel::Error), Some(&message))
                .execute_and_expect(ReturnType::Bool(false))?;
        }
    }
    Ok(())
}

#[test]
#[serial]
fn reconfiguration_accepts_both_formats_and_preserves_pipeline_on_error() -> TestResult {
    let mut module = module()?;
    configure(&mut module, GO_HEADERS);
    module
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))?;
    let invalid = br#"{"encoding":"gzip+base64"}"#;
    module
        .call_proxy_on_configure(1, invalid.len() as i32)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(invalid))
        .expect_log(
            Some(LogLevel::Error),
            Some("failed to load plugin config: envelope requires both encoding and data"),
        )
        .execute_and_expect(ReturnType::Bool(false))?;
    assert_header_change(&mut module, 2)?;
    module
        .call_proxy_on_configure(1, HEADERS.len() as i32)
        .expect_log(Some(LogLevel::Info), Some("#1 on_configure"))
        .expect_increment_metric(Some(1), Some(1))
        .expect_get_buffer_bytes(Some(BufferType::PluginConfiguration))
        .returning(Some(HEADERS))
        .expect_get_log_level()
        .returning(Some(LOG_LEVEL))
        .execute_and_expect(ReturnType::Bool(true))?;
    assert_header_change(&mut module, 3)?;
    Ok(())
}
