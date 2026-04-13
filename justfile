# aionrs justfile — run tasks with `vx just <recipe>`
# All commands route through `vx` so the correct tool versions are used.

# Cross-platform shell defaults for linewise recipes.
set shell := ["sh", "-cu"]
set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

# Default: list all recipes
default:
    @vx just --list

# ── Build ──────────────────────────────────────────────────────────────────
build:
    vx cargo build --workspace

build-release:
    vx cargo build --workspace --release

# ── Test ───────────────────────────────────────────────────────────────────

# Unit + integration tests with nextest (default profile — local dev)
test:
    vx cargo nextest run --workspace --profile default

# Unit + integration tests with nextest (CI profile — used in GitHub Actions)
test-ci:
    vx cargo nextest run --workspace --profile ci

# Run a single test by name
test-one NAME:
    vx cargo nextest run --workspace -E 'test({{ NAME }})'

# Show test output (debug failing tests locally)
test-verbose:
    vx cargo nextest run --workspace --profile default --no-capture

# ── E2E Tests ──────────────────────────────────────────────────────────────
# Requires env vars: ANTHROPIC_API_KEY and/or OPENAI_API_KEY
# Uses the dedicated e2e nextest profile (sequential, long timeout, no retry)
test-e2e:
    vx cargo nextest run --workspace --profile e2e --test e2e

test-e2e-anthropic:
    vx cargo nextest run -p aion-agent --profile e2e --test e2e -E 'test(anthropic)'

test-e2e-openai:
    vx cargo nextest run -p aion-agent --profile e2e --test e2e -E 'test(openai)'

# ── Acceptance Tests (evolution feature validation) ───────────────────────
# Requires env vars: OPENAI_API_KEY and/or AWS_PROFILE + CLAUDE_CODE_USE_BEDROCK=1
# Reuses the e2e nextest profile (sequential, long timeout, no retry)
test-acceptance:
    vx cargo nextest run -p aion-agent --profile e2e --test acceptance

test-acceptance-memory:
    vx cargo nextest run -p aion-agent --profile e2e --test acceptance -E 'test(memory)'

test-acceptance-compact:
    vx cargo nextest run -p aion-agent --profile e2e --test acceptance -E 'test(compact)'

# ── Lint / Format ─────────────────────────────────────────────────────────
lint:
    vx cargo clippy --workspace --all-targets -- -D warnings

fmt:
    vx cargo fmt --all

fmt-check:
    vx cargo fmt --all -- --check

# ── Workspace-hack (cargo-hakari) ─────────────────────────────────────────
hakari-generate:
    vx cargo hakari generate

hakari-verify:
    vx cargo hakari verify

# ── Security ──────────────────────────────────────────────────────────────
audit:
    vx cargo audit

# ── Coverage ──────────────────────────────────────────────────────────────
coverage:
    vx cargo llvm-cov nextest --workspace --profile ci --lcov --output-path lcov.info

# ── Release ───────────────────────────────────────────────────────────────
aion_version := `vx cargo pkgid -p aion-cli | sed 's/.*#//'`

version:
    @echo '{{ aion_version }}'

# ── Clean ─────────────────────────────────────────────────────────────────
clean:
    vx cargo clean

# ── Pre-push gate (format, lint, test, then push) ────────────────────────
push *ARGS:
    vx cargo fmt --all
    vx cargo clippy --workspace --all-targets -- -D warnings
    vx cargo test --workspace
    git push {{ ARGS }}

# ── All checks (mirrors CI exactly) ───────────────────────────────────────
check-all: fmt-check lint test-ci hakari-verify audit
