build-wasm:
    cargo run --locked --manifest-path xtask/Cargo.toml -- build-wasm

test:
    cargo test --locked

lint:
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets --all-features -- -D warnings
