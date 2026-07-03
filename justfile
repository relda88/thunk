fmt:
    cargo fmt --all

check:
    cargo check --all-targets

clippy:
    cargo clippy --all-targets

test:
    cargo test --no-default-features

verify:
    cargo fmt --all --check
    cargo check --all-targets
    cargo clippy --all-targets
    cargo test --no-default-features

run:
    cargo run --release

trace:
    THUNK_TRACE_RUNTIME=1 cargo run --release

fresh:
    rm -f data/sessions.db

trace-fresh:
    just fresh
    just trace

frontend-install:
    cd frontend && npm install

frontend-build:
    cd frontend && npm run build

gui: frontend-build
    cargo run --features gui -- --gui

gui-trace: frontend-build
    THUNK_TRACE_RUNTIME=1 cargo run --features gui -- --gui

gui-release: frontend-build
    cargo run --release --features gui -- --gui

gui-trace-release: frontend-build
    THUNK_TRACE_RUNTIME=1 cargo run --release --features gui -- --gui

gui-dev:
    cargo tauri dev --features gui -- -- --gui

install:
    cargo install --path .

clean-logs:
    rm -f logs/*
