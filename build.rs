use std::process::Command;
use std::env;

fn main() {
    // Very bad build.rs that just rerun yarn run build when building in release mode
    let out_dir = env::var_os("PROFILE").unwrap();
    if out_dir == "release" {
        println!("cargo::warning=Frontend compilation started");
        Command::new("yarn").current_dir("frontend").args(["run", "build"])
            .status().unwrap();
        println!("cargo::warning=Frontend compilation completed");
    } else {
        println!("cargo::warning=Frontend compilation skipped, it's needed only in release mode");
    }
    println!("cargo::rerun-if-changed=frontend/src");
}
