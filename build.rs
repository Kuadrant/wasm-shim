use std::error::Error;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    set_git_hash("WASM_SHIM_GIT_HASH");
    set_profile("WASM_SHIM_PROFILE");
    set_features("WASM_SHIM_FEATURES");
    generate_protobuf()
}

fn set_profile(env: &str) {
    if let Ok(profile) = std::env::var("PROFILE") {
        println!("cargo:rustc-env={env}={profile}");
    }
}

fn set_features(env: &str) {
    let mut features = vec![];
    if cfg!(feature = "with-serde") {
        features.push("+with-serde");
    }
    println!("cargo:rustc-env={env}={features:?}");
}

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
