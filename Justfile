# Every gate to pass before a hand-over is `just check`. Each recipe also runs on its own.

# List the recipes
default:
    @{{just_executable()}} --list

# Every gate before a hand-over, in order: test, lint, the browser build's check
check: test lint check-wasm

# The suite
test:
    cargo test

# Clippy on every target with warnings denied, then the formatting check
lint:
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

# The browser build compiles, and clippy passes on it with warnings denied
check-wasm:
    cargo check --target wasm32-unknown-unknown
    cargo clippy --target wasm32-unknown-unknown -- -D warnings

# Format in place
fmt:
    cargo fmt

# Run the app against the hub on 127.0.0.1:8765
run:
    cargo run

# The browser bundle in dist/, a release build shrunk with wasm-opt -Oz
wasm:
    trunk build --release

# Serve the browser build on 127.0.0.1:PORT, passing /api and /ws on to the hub
serve port="8080":
    trunk serve --port {{port}}
