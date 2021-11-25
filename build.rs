fn main() -> Result<(), Box<dyn std::error::Error>> {
    protoc_rust::Codegen::new()
        .out_dir("src/envoy_ext_auth/")
        .inputs(&[
            "vendor-protobufs/data-plane-api/envoy/service/auth/v3/external_auth.proto",
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
        ])
        .includes(&[
            "vendor-protobufs/data-plane-api/",
            "vendor-protobufs/protoc-gen-validate/",
            "vendor-protobufs/udpa/",
            "vendor-protobufs/googleapis/",
        ])
        .run()
        .expect("running protoc failed");
    Ok(())
}
