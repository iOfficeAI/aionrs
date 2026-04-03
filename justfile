# aionrs justfile — run tasks with `vx just <recipe>`

# Default: list all recipes
default:
    @vx just --list

# ── Build ──────────────────────────────────────────────────────────────────
build:
    vx cargo build --workspace

build-release:
    vx cargo build --workspace --release

# ── Test ───────────────────────────────────────────────────────────────────
test:
    vx cargo test --workspace

test-verbose:
    vx cargo test --workspace -- --nocapture

# Run tests with nextest (faster parallel runner)
nextest:
    vx cargo nextest run --workspace

# ── Lint / Format ─────────────────────────────────────────────────────────
lint:
    vx cargo clippy --workspace --all-targets -- -D warnings

fmt:
    vx cargo fmt --all

fmt-check:
    vx cargo fmt --all -- --check

# ── Workspace-hack (cargo-hakari) ─────────────────────────────────────────
# Regenerate workspace-hack after adding/changing dependencies
hakari-generate:
    vx cargo hakari generate

# Verify workspace-hack is up-to-date (run in CI)
hakari-verify:
    vx cargo hakari verify

# ── Security ──────────────────────────────────────────────────────────────
audit:
    vx cargo audit

# ── Release ───────────────────────────────────────────────────────────────
# Show the current workspace version
version:
    @vx cargo metadata --no-deps --format-version 1 | vx python -c "import sys,json; d=json.load(sys.stdin); print(d['packages'][0]['version'])"

# ── Clean ─────────────────────────────────────────────────────────────────
clean:
    vx cargo clean

# ── All checks (mirrors CI) ───────────────────────────────────────────────
check-all: fmt-check lint test hakari-verify
