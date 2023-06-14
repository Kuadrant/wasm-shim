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
        .protoc_path("bin/protoc")
        .out_dir("src/envoy")
        .customize(custom)
        .inputs(["vendor-protobufs/data-plane-api/envoy/service/ratelimit/v3/rls.proto"])
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
