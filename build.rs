use std::error::Error;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    set_git_hash("WASM_SHIM_GIT_HASH");
    set_profile("WASM_SHIM_PROFILE");
    set_features("WASM_SHIM_FEATURES");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor-protobufs");
    generate_protobuf()
}

fn set_profile(env: &str) {
    if let Ok(profile) = std::env::var("PROFILE") {
        println!("cargo:rustc-env={env}={profile}");
    }
}

fn set_features(env: &str) {
    let features: Vec<&str> = vec![];
    println!("cargo:rustc-env={env}={features:?}");
}

#[allow(clippy::indexing_slicing)]
fn set_git_hash(env: &str) {
    let git_sha = Command::new("/usr/bin/git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .map(|sha| sha[..8].to_owned());

    if let Some(sha) = git_sha {
        let dirty = Command::new("/usr/bin/git")
            .args(["diff", "--stat"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| !matches!(output.stdout.len(), 0));

        match dirty {
            Some(true) => println!("cargo:rustc-env={env}={sha}-dirty"),
            Some(false) => println!("cargo:rustc-env={env}={sha}"),
            _ => unreachable!("How can we have a git hash, yet not know if the tree is dirty?"),
        }
    } else {
        let fallback = option_env!("GITHUB_SHA")
            .map(|sha| if sha.len() > 8 { &sha[..8] } else { sha })
            .unwrap_or("NO_SHA");
        println!("cargo:rustc-env={env}={fallback}");
    }
}

#[allow(clippy::expect_used)]
fn generate_protobuf() -> Result<(), Box<dyn Error>> {
    println!("Starting protobuf generation...");

    let out_dir = std::env::var("OUT_DIR")?;

    // Generate Rust code for all protos
    let mut prost_build = prost_build::Config::new();
    prost_build.out_dir("src/envoy");

    println!("Compiling protos...");
    let result = prost_build.compile_protos(
        &["vendor-protobufs/kuadrant/v1/descriptor_service.proto"],
        &["vendor-protobufs/kuadrant/"],
    );

    match &result {
        Ok(_) => println!("Protobuf generation completed successfully!"),
        Err(e) => println!("cargo:error=Protobuf generation failed: {}", e),
    }

    result?;

    // Generate separate FileDescriptorSets for embedded services
    println!("Generating embedded descriptors...");

    // RateLimit service descriptors (both envoy and kuadrant services, same messages)
    let mut ratelimit_config = prost_build::Config::new();
    ratelimit_config.file_descriptor_set_path(format!("{}/ratelimit_descriptors.bin", out_dir));
    ratelimit_config.compile_protos(
        &[
            "vendor-protobufs/data-plane-api/envoy/service/ratelimit/v3/rls.proto",
            "vendor-protobufs/kuadrant/service/ratelimit/v1/ratelimit.proto",
        ],
        &[
            "vendor-protobufs/data-plane-api/",
            "vendor-protobufs/protoc-gen-validate/",
            "vendor-protobufs/udpa/",
            "vendor-protobufs/xds/",
            "vendor-protobufs/googleapis/",
            "vendor-protobufs/kuadrant/",
        ],
    )?;

    // Auth service descriptors (don't generate Rust code, just descriptor set)
    let mut auth_config = prost_build::Config::new();
    auth_config.file_descriptor_set_path(format!("{}/auth_descriptors.bin", out_dir));
    auth_config.compile_protos(
        &["vendor-protobufs/data-plane-api/envoy/service/auth/v3/external_auth.proto"],
        &[
            "vendor-protobufs/data-plane-api/",
            "vendor-protobufs/protoc-gen-validate/",
            "vendor-protobufs/udpa/",
            "vendor-protobufs/xds/",
            "vendor-protobufs/googleapis/",
        ],
    )?;

    println!("Embedded descriptor generation completed!");
    Ok(())
}
