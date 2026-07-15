use std::process::Command;

fn main() {
    set_git_hash("WASM_SHIM_GIT_HASH");
    set_profile("WASM_SHIM_PROFILE");
    set_features("WASM_SHIM_FEATURES");
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
