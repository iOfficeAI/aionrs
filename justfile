# aionrs justfile — run tasks with `vx just <recipe>`

# Cross-platform shell defaults for linewise recipes.
# - Unix-like: POSIX sh
# - Windows: PowerShell Core
set shell := ["sh", "-cu"]
set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

# Default: list all recipes
default:
    @just --list

# ── Build ──────────────────────────────────────────────────────────────────
build:
    cargo build --workspace

build-release:
    cargo build --workspace --release

# ── Test ───────────────────────────────────────────────────────────────────

# Unit + integration tests with nextest (default profile — local dev)
test:
    cargo nextest run --workspace --profile default

# Unit + integration tests with nextest (CI profile — used in GitHub Actions)
test-ci:
    cargo nextest run --workspace --profile ci

# Run a single test by name
test-one NAME:
    cargo nextest run --workspace -E 'test({{ NAME }})'

# Show test output (debug failing tests locally)
test-verbose:
    cargo nextest run --workspace --profile default --no-capture

# ── E2E Tests ──────────────────────────────────────────────────────────────
# Requires env vars: ANTHROPIC_API_KEY and/or OPENAI_API_KEY
# Uses the dedicated e2e nextest profile (sequential, long timeout, no retry)
test-e2e:
    cargo nextest run --workspace --profile e2e --test e2e

test-e2e-anthropic:
    cargo nextest run -p aion-agent --profile e2e --test e2e -E 'test(anthropic)'

test-e2e-openai:
    cargo nextest run -p aion-agent --profile e2e --test e2e -E 'test(openai)'

# ── Lint / Format ─────────────────────────────────────────────────────────
lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# ── Workspace-hack (cargo-hakari) ─────────────────────────────────────────
hakari-generate:
    cargo hakari generate

hakari-verify:
    cargo hakari verify

# ── Security ──────────────────────────────────────────────────────────────
audit:
    cargo audit

# ── Coverage ──────────────────────────────────────────────────────────────
coverage:
    cargo llvm-cov nextest --workspace --profile ci --lcov --output-path lcov.info

# ── Release ───────────────────────────────────────────────────────────────
aion_version := replace_regex(`cargo pkgid -p aion-cli`, '^.*#', '')

version:
    @echo '{{ aion_version }}'


# ── Clean ─────────────────────────────────────────────────────────────────
clean:
    cargo clean

# ── All checks (mirrors CI exactly) ───────────────────────────────────────
check-all: fmt-check lint test-ci hakari-verify audit
