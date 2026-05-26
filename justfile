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

install:
    cargo install --path .

clean-logs:
    rm -f logs/*