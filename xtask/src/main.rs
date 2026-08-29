use std::{env, fs, path::PathBuf, process::Command};

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const WASM_ARTIFACT: &str = "vanilla_aruco_wasm.wasm";
const PLUGIN_FILE: &str = "aruco.wasm";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live directly below the workspace root")
        .to_path_buf()
}

fn run(root: &PathBuf, program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .unwrap_or_else(|error| panic!("failed to start {program}: {error}"));
    if !status.success() {
        panic!("{program} exited with {status}");
    }
}

fn build_wasm(root: &PathBuf) {
    run(root, "rustup", &["target", "add", WASM_TARGET]);
    run(
        root,
        "cargo",
        &[
            "build",
            "--locked",
            "--release",
            "--target",
            WASM_TARGET,
            "-p",
            "vanilla-aruco-wasm",
        ],
    );

    let source = root
        .join("target")
        .join(WASM_TARGET)
        .join("release")
        .join(WASM_ARTIFACT);
    let destination = root.join(PLUGIN_FILE);
    fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    println!("copied {} to {}", source.display(), destination.display());
}

fn main() {
    let command = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: cargo run --manifest-path xtask/Cargo.toml -- build-wasm");
        std::process::exit(2);
    });
    let root = workspace_root();
    match command.as_str() {
        "build-wasm" => build_wasm(&root),
        other => panic!("unknown xtask command: {other}"),
    }
}
