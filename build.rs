use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    generate_protobuf()
}

fn generate_protobuf() -> Result<(), Box<dyn Error>> {
    let custom = protoc_rust::Customize {
        serde_derive: Some(true),
        ..Default::default()
    };
    protoc_rust::Codegen::new()
        .out_dir("src/envoy")
        .customize(custom)
        .inputs([
            "vendor-protobufs/data-plane-api/envoy/service/auth/v3/external_auth.proto",
            "vendor-protobufs/data-plane-api/envoy/service/ratelimit/v3/rls.proto",
            "vendor-protobufs/data-plane-api/envoy/service/auth/v3/attribute_context.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/address.proto",
            "vendor-protobufs/googleapis/google/protobuf/timestamp.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/base.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/socket_option.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/http_uri.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/backoff.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/http_status.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/semantic_version.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/percent.proto",
            "vendor-protobufs/udpa/xds/core/v3/context_params.proto",
            "vendor-protobufs/googleapis/google/rpc/status.proto",
            "vendor-protobufs/data-plane-api/envoy/config/route/v3/route_components.proto",
            "vendor-protobufs/data-plane-api/envoy/extensions/common/ratelimit/v3/ratelimit.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/ratelimit_unit.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/range.proto",
            "vendor-protobufs/data-plane-api/envoy/type/v3/token_bucket.proto",
            "vendor-protobufs/data-plane-api/envoy/config/common/matcher/v3/matcher.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/extension.proto",
            "vendor-protobufs/data-plane-api/envoy/type/matcher/v3/string.proto",
            "vendor-protobufs/data-plane-api/envoy/type/matcher/v3/number.proto",
            "vendor-protobufs/data-plane-api/envoy/type/matcher/v3/regex.proto",
            "vendor-protobufs/data-plane-api/envoy/type/matcher/v3/metadata.proto",
            "vendor-protobufs/data-plane-api/envoy/type/matcher/v3/value.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/config_source.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/grpc_service.proto",
            "vendor-protobufs/data-plane-api/envoy/config/core/v3/proxy_protocol.proto",
            "vendor-protobufs/udpa/xds/core/v3/authority.proto",
            "vendor-protobufs/data-plane-api/envoy/type/tracing/v3/custom_tag.proto",
        ])
        .includes([
            "vendor-protobufs/data-plane-api/",
            "vendor-protobufs/protoc-gen-validate/",
            "vendor-protobufs/udpa/",
            "vendor-protobufs/xds/",
            "vendor-protobufs/googleapis/",
        ])
        .run()
        .expect("running protoc failed");
    Ok(())
}
