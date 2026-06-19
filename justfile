# aionrs justfile — run tasks with `vx just <recipe>`
# All commands route through `vx` (when available) so the correct tool
# versions are used. Everything here is cross-platform: recipe bodies avoid
# shell builtins and external Unix tools (no printf/sed), relying on just's
# own functions instead, so the same justfile works on macOS, Linux & Windows.

# Cross-platform shell defaults for linewise recipes.
set shell := ["sh", "-cu"]
set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

# `which()` is used below to probe for `vx`; it is a just unstable feature.
set unstable

# Probe for `vx` once at load time, using just's own (cross-platform) `which`
# rather than a shell builtin. If present, commands run through it to pin tool
# versions; if not, this expands to empty and commands run bare.
vx := if which("vx") == "" { "" } else { "vx" }

# Route cargo through vx when available — acts like `alias cargo = vx cargo`
# scoped to this justfile. Recipes just write `{{ cargo }} ...`.
cargo := trim(vx + " cargo")

# Bold-cyan / reset ANSI codes for the colored command echo in the unix `_run`.
# (just's `style("command")` only emits bold — no color — so we spell it out.)
CYAN := "\u{1b}[1;36m"
NORMAL := "\u{1b}[0m"

# Default: list all recipes
default:
    @{{ vx }} just --list

# Echo a command in bold cyan, then run it. Every action recipe routes through
# this so the coloring lives in one place. The command is passed as ONE quoted
# string to preserve embedded quotes (e.g. -E 'test(...)'). Split per-OS: unix
# emits raw ANSI via `printf`, Windows uses pwsh's native colored `Write-Host`
# (more reliable than ANSI on older Windows consoles).
[unix]
_run cmd:
    @printf '%s\n' "{{ CYAN }}{{ cmd }}{{ NORMAL }}"
    @{{ cmd }}

[windows]
_run cmd:
    @Write-Host "{{ cmd }}" -ForegroundColor Cyan
    @{{ cmd }}

# ── Build ──────────────────────────────────────────────────────────────────
build:
    @just _run "{{ cargo }} build --workspace"

build-release:
    @just _run "{{ cargo }} build --workspace --release"

# ── Test ───────────────────────────────────────────────────────────────────

# Unit + integration tests with nextest (default profile — local dev)
test:
    @just _run "{{ cargo }} nextest run --workspace --profile default"

# Unit + integration tests with nextest (CI profile — used in GitHub Actions)
test-ci:
    @just _run "{{ cargo }} nextest run --workspace --profile ci"

# Run a single test by name
test-one NAME:
    @just _run "{{ cargo }} nextest run --workspace -E 'test({{ NAME }})'"

# Show test output (debug failing tests locally)
test-verbose:
    @just _run "{{ cargo }} nextest run --workspace --profile default --no-capture"

# ── E2E Tests ──────────────────────────────────────────────────────────────
# Requires env vars: ANTHROPIC_API_KEY and/or OPENAI_API_KEY
# Uses the dedicated e2e nextest profile (sequential, long timeout, no retry)
test-e2e:
    @just _run "{{ cargo }} nextest run --workspace --profile e2e --test e2e"

test-e2e-anthropic:
    @just _run "{{ cargo }} nextest run -p aion-agent --profile e2e --test e2e -E 'test(anthropic)'"

test-e2e-openai:
    @just _run "{{ cargo }} nextest run -p aion-agent --profile e2e --test e2e -E 'test(openai)'"

# ── Acceptance Tests (evolution feature validation) ───────────────────────
# Requires env vars: OPENAI_API_KEY and/or AWS_PROFILE + CLAUDE_CODE_USE_BEDROCK=1
# Reuses the e2e nextest profile (sequential, long timeout, no retry)
test-acceptance:
    @just _run "{{ cargo }} nextest run -p aion-agent --profile e2e --test acceptance"

test-acceptance-memory:
    @just _run "{{ cargo }} nextest run -p aion-agent --profile e2e --test acceptance -E 'test(memory)'"

test-acceptance-compact:
    @just _run "{{ cargo }} nextest run -p aion-agent --profile e2e --test acceptance -E 'test(compact)'"

# ── Lint / Format ─────────────────────────────────────────────────────────
lint:
    @just _run "{{ cargo }} clippy --workspace --all-targets -- -D warnings"

lint-fix:
    @just _run "{{ cargo }} fix --allow-dirty --allow-staged"
    @just _run "{{ cargo }} clippy --fix --workspace --all-targets --allow-dirty --allow-staged -- -D warnings"

fmt:
    @just _run "{{ cargo }} fmt --all"

fmt-check:
    @just _run "{{ cargo }} fmt --all -- --check"

# ── Workspace-hack (cargo-hakari) ─────────────────────────────────────────
hakari-generate:
    @just _run "{{ cargo }} hakari generate"

hakari-verify:
    @just _run "{{ cargo }} hakari verify"

# ── Security ──────────────────────────────────────────────────────────────
audit:
    @just _run "{{ cargo }} audit"

# ── Coverage ──────────────────────────────────────────────────────────────
coverage:
    @just _run "{{ cargo }} llvm-cov nextest --workspace --profile ci --lcov --output-path lcov.info"

# ── Release ───────────────────────────────────────────────────────────────
# `cargo pkgid` prints `...#<version>`; strip everything up to and including
# the `#`. No `sed` (absent on Windows) — use each shell's native facility.
[unix]
version:
    @{{ cargo }} pkgid -p aion-cli | sed 's/.*#//'

[windows]
version:
    @({{ cargo }} pkgid -p aion-cli) -replace '.*#'

# ── Clean ─────────────────────────────────────────────────────────────────
clean:
    @just _run "{{ cargo }} clean"

# ── Pre-push gate (lint-fix, format, auto-commit fixes, test, then push) ─
push *ARGS: lint-fix fmt _auto-commit-fixes test
    git push {{ ARGS }}

# Auto-commit any fmt/clippy fixes. Pure git + the `||` chain operator, which
# both `sh` and `pwsh` (7+) understand — so no bash shebang / `[ ]` test, and
# it runs the same on every OS. `git diff --cached --quiet` exits non-zero only
# when there is something staged, gating the commit.
_auto-commit-fixes:
    @git add -A
    @git diff --cached --quiet || git commit -m "chore: auto-commit lint/fmt fixes in just push recipe"

# ── All checks (mirrors CI exactly) ───────────────────────────────────────
check-all: fmt-check lint test-ci hakari-verify audit
