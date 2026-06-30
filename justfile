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

gui:
    cargo run --features gui -- --gui

gui-trace:
    THUNK_TRACE_RUNTIME=1 cargo run --features gui -- --gui

gui-release:
    cargo run --release --features gui -- --gui

gui-trace-release:
    THUNK_TRACE_RUNTIME=1 cargo run --release --features gui -- --gui

install:
    cargo install --path .

clean-logs:
    rm -f logs/*