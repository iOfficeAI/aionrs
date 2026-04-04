---
name: vx-best-practices
description: "Best practices for using vx effectively. Use when following recommended patterns for tool management, project setup, and team workflows with vx."
---

# VX Best Practices

## General Principles

### 1. Always Use `vx` Prefix

In vx-managed projects, always prefix tool commands with `vx`:

```bash
# ✅ Correct
vx npm install
vx cargo build
vx just test

# ❌ Wrong (might use system tools)
npm install
cargo build
just test
```

### 2. Prefer Project-Level Configuration

Use `vx.toml` to ensure consistency across team members:

```bash
# ✅ Correct - defined in vx.toml
vx sync

# ❌ Wrong - manual installation
vx install node@22
```

### 3. Commit Lock Files

Always commit `vx.lock` to ensure reproducible builds:

```bash
git add vx.lock
git commit -m "chore: update dependencies"
```

## Project Setup

### Initial Setup

```bash
# 1. Initialize project
vx init

# 2. Add required tools
vx add node@22
vx add go
vx add just

# 3. Generate lock file
vx lock

# 4. Commit configuration
git add vx.toml vx.lock
```

### Team Onboarding

New team members only need to run:

```bash
# Clone repository
git clone <repo>
cd <repo>

# One command setup
vx setup
```

### CI/CD Configuration

Use the vx GitHub Action:

```yaml
# .github/workflows/ci.yml
- uses: loonghao/vx@main
  with:
    setup: 'true'
    cache: 'true'
```

## Version Management

### Version Selection Strategy

| Scenario | Constraint | Example |
|----------|------------|---------|
| Development | Major version | `node = "22"` |
| CI/CD | Exact version | `node = "22.0.0"` |
| Library | Range | `node = ">=18 <23"` |
| Latest features | `"latest"` | `uv = "latest"` |

### Avoid Over-Specification

```toml
# ✅ Good - flexible for patch updates
[tools]
node = "22"
go = "1.22"

# ❌ Bad - too rigid for development
[tools]
node = "22.0.0"    # Blocks security patches
go = "1.22.0"      # Requires update for each patch
```

### LTS for Production

```toml
# Use LTS versions for stability
[tools]
node = "lts"        # Auto-updates to latest LTS
```

## Scripts Organization

### Naming Conventions

```toml
[scripts]
# Standard scripts (run with: vx run <name>)
dev = "npm run dev"
test = "npm run test"
build = "npm run build"
lint = "npm run lint"

# Colon-separated variants (run with: vx run test:watch)
test:watch = "npm run test -- --watch"
test:coverage = "npm run test -- --coverage"
build:prod = "npm run build -- --mode production"

# CI-specific scripts
ci = "just ci"
ci:test = "cargo test --all-features"
```

### Script Dependencies

```toml
# Scripts can depend on specific tools
[scripts]
lint = "eslint . && cargo clippy"  # Uses node and rust
build = "go build ./..."           # Uses go
```

## Environment Variables

### Project-Level Defaults

```toml
[env]
NODE_ENV = "development"
DEBUG = "app:*"

# Required variables (vx will warn if missing)
API_KEY = { env = "API_KEY", required = true }

# Default values
PORT = { default = "3000" }
```

### Secrets Management

Never commit secrets. Use environment variables:

```bash
# .env (add to .gitignore)
DATABASE_URL=postgresql://...
API_KEY=secret123

# Reference in vx.toml
[env]
DATABASE_URL = { env = "DATABASE_URL" }
```

## Performance Optimization

### Cache Configuration

```toml
[cache]
# Enable aggressive caching
enabled = true
ttl = 86400  # 24 hours

# Cache location
dir = "~/.vx/cache"
```

### Pre-install Common Tools

```bash
# In project setup, pre-install tools
vx sync --parallel
```

### Use Offline Mode

```bash
# When network is unreliable
vx sync --offline
```

## Cross-Platform Considerations

### Platform-Specific Tools

```toml
[tools]
# Cross-platform tools first
node = "22"
uv = "latest"

# Platform-specific
[tools.msvc]
version = "14.42"
os = ["windows"]

[tools.brew]
version = "latest"
os = ["macos", "linux"]
```

### Cross-Platform Scripts

```bash
# Use tools that work everywhere
[scripts]
build = "just build"     # just is cross-platform
test = "cargo test"      # cargo is cross-platform

# Avoid platform-specific commands
# ❌ build = "make build"  # Unix only
```

## Security Best Practices

### Verify Checksums

vx automatically verifies checksums. For additional security:

```bash
# Verify installation
vx install node@22 --verify
```

### Minimal Permissions

```bash
# Install as regular user, not root
# ❌ sudo vx install node

# vx manages user-level installations
vx install node
```

### Audit Dependencies

```bash
# Audit installed tools
vx audit

# Check for vulnerabilities
vx npm audit
```

## Team Workflows

### Adding New Tools

```bash
# 1. Add to vx.toml
vx add python@3.12

# 2. Update lock file
vx lock

# 3. Commit changes
git add vx.toml vx.lock
git commit -m "feat: add python 3.12"
```

### Updating Tools

```bash
# 1. Check for updates
vx outdated

# 2. Update specific tool
vx update node

# 3. Update all tools
vx update --all

# 4. Commit lock file changes
git add vx.lock
```

### Removing Tools

```bash
# 1. Remove from vx.toml
vx remove tool-name

# 2. Clean up installation
vx sync --clean

# 3. Commit changes
git add vx.toml vx.lock
```

## Monitoring & Maintenance

### Regular Checks

```bash
# Weekly: Check for updates
vx outdated

# Monthly: Clean cache
vx cache clean

# After issues: Run diagnostics
vx doctor
```

### Health Check Script

```toml
[scripts]
doctor = "vx doctor"
check = "vx check"
audit = "vx npm audit && cargo audit"
```

## Anti-Patterns to Avoid

### 1. Manual Tool Installation

```bash
# ❌ Don't manually install tools in vx projects
npm install -g node  # Conflicts with vx

# ✅ Let vx manage tools
vx npm install
```

### 2. Ignoring Lock Files

```bash
# ❌ Don't ignore vx.lock
git rm --cached vx.lock

# ✅ Always commit lock files
git add vx.lock
```

### 3. Global vx.toml

```bash
# ❌ Don't rely on global configuration
~/.vx/vx.toml

# ✅ Use project-level configuration
./vx.toml
```

### 4. Hardcoded Paths

```toml
# ❌ Don't hardcode absolute paths
[env]
TOOL_PATH = "/Users/alice/.vx/tools/node"

# ✅ Use vx variables
[env]
TOOL_PATH = "${VX_ROOT}/tools/node"
```

## Migration Guide

### From nvm/fnm → vx

```bash
# 1. Export current versions
node --version > .nvmrc

# 2. Create vx.toml
vx init

# 3. Add node version
vx add node@$(cat .nvmrc | tr -d 'v')

# 4. Remove nvm
rm -rf ~/.nvm
```

### From pyenv → vx

```bash
# 1. Check current Python version
python --version

# 2. Create vx.toml
vx init

# 3. Add uv (recommended Python manager)
vx add uv

# 4. Remove pyenv
rm -rf ~/.pyenv
```

## Provider Development Best Practices

### Choose the Right Template

For most tools, use a template instead of writing custom functions:

```starlark
# ✅ Good — Use template (10 lines)
_p = github_rust_provider("owner", "tool",
    asset = "tool-{vversion}-{triple}.{ext}")

# ❌ Avoid — Custom download_url when template works
def download_url(ctx, version):
    # 30+ lines of custom code...
```

### Provider Naming

```starlark
# ✅ Correct terminology
name = "ripgrep"              # Provider name
runtimes = [runtime_def("rg")]  # Runtime name (what user types)

# Common pattern: provider name = project name, runtime name = binary name
# ripgrep provider → rg runtime
# rust provider → cargo, rustc, rustup runtimes
```

### Platform Constraints

Return `None` from `download_url` for unsupported platforms:

```starlark
def download_url(ctx, version):
    platform = platform_map(ctx, _PLATFORMS)
    if not platform:
        return None  # Not supported on this platform
    return "https://example.com/v{}/tool-{}.tar.gz".format(version, platform)
```

### Bundled Runtimes

Use `bundled_runtime_def` for tools shipped inside another:

```starlark
runtimes = [
    runtime_def("node"),                     # Primary runtime
    bundled_runtime_def("npm", "node"),       # npm comes with node
    bundled_runtime_def("npx", "node"),       # npx comes with node
]
```

## vx Development Best Practices

### Quick Development Cycle

```bash
vx just quick                    # format → lint → test → build
vx cargo check -p vx-cli         # Fast type-checking for one crate
vx cargo test -p vx-starlark     # Test one crate
```

### Code Organization Rules

1. **Layer dependencies go downward only** — Never import from higher layers
2. **Tests in `tests/` directories** — Never inline `#[cfg(test)]`
3. **Use `rstest`** for parameterized tests
4. **Use `tracing`** for logging, never `println!` or `eprintln!`
5. **Use correct terminology** — Runtime, Provider, provider.star
