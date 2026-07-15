use std::error::Error;
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../vendor-protobufs");
    generate_protobuf()
}

#[allow(clippy::expect_used)]
fn generate_protobuf() -> Result<(), Box<dyn Error>> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root must exist");
    let vendor = workspace_root.join("vendor-protobufs");

    let out_dir = std::env::var("OUT_DIR")?;

    let mut prost_build = prost_build::Config::new();
    prost_build.out_dir("src/proto");

    let result = prost_build.compile_protos(
        &[vendor
            .join("kuadrant/v1/descriptor_service.proto")
            .to_str()
            .expect("valid path")],
        &[vendor.join("kuadrant/").to_str().expect("valid path")],
    );

    match &result {
        Ok(_) => println!("Protobuf generation completed successfully!"),
        Err(e) => println!("cargo:error=Protobuf generation failed: {}", e),
    }

    result?;

    let mut ratelimit_config = prost_build::Config::new();
    ratelimit_config.file_descriptor_set_path(format!("{}/ratelimit_descriptors.bin", out_dir));
    ratelimit_config.compile_protos(
        &[
            vendor
                .join("data-plane-api/envoy/service/ratelimit/v3/rls.proto")
                .to_str()
                .expect("valid path"),
            vendor
                .join("kuadrant/service/ratelimit/v1/ratelimit.proto")
                .to_str()
                .expect("valid path"),
        ],
        &[
            vendor.join("data-plane-api/").to_str().expect("valid path"),
            vendor
                .join("protoc-gen-validate/")
                .to_str()
                .expect("valid path"),
            vendor.join("udpa/").to_str().expect("valid path"),
            vendor.join("xds/").to_str().expect("valid path"),
            vendor.join("googleapis/").to_str().expect("valid path"),
            vendor.join("kuadrant/").to_str().expect("valid path"),
        ],
    )?;

    let mut auth_config = prost_build::Config::new();
    auth_config.file_descriptor_set_path(format!("{}/auth_descriptors.bin", out_dir));
    auth_config.compile_protos(
        &[vendor
            .join("data-plane-api/envoy/service/auth/v3/external_auth.proto")
            .to_str()
            .expect("valid path")],
        &[
            vendor.join("data-plane-api/").to_str().expect("valid path"),
            vendor
                .join("protoc-gen-validate/")
                .to_str()
                .expect("valid path"),
            vendor.join("udpa/").to_str().expect("valid path"),
            vendor.join("xds/").to_str().expect("valid path"),
            vendor.join("googleapis/").to_str().expect("valid path"),
        ],
    )?;

    Ok(())
}
