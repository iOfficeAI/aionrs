---
name: vx-project
description: "Project management guide for vx. Use when setting up a new project, configuring vx.toml, or managing project-level tool versions and scripts."
---

# VX Project Management Guide

## Project Setup

### Initialize a Project

```bash
vx init                     # Create vx.toml interactively
vx init --template node     # Use a template
vx init --minimal           # Create minimal vx.toml
```

### Project Detection

vx automatically detects project types and suggests tools:

```bash
vx analyze                  # Analyze project (detects languages, dependencies)
vx analyze --json           # JSON output for AI parsing
```

**Detected ecosystems**: Node.js, Python, Rust, Go, Java, .NET, C/C++, Zig
**Detected frameworks**: React, Vue, Angular, Next.js, Nuxt, Svelte, Django, Flask, FastAPI, Tauri, Electron, React Native, NW.js, and more
**Detected package managers**: npm, yarn, pnpm, bun, pip, uv, cargo, go modules

The project analyzer reads indicator files like `package.json`, `pyproject.toml`, `Cargo.toml`, `go.mod`, etc. to suggest the right tools.

## vx.toml Configuration

### Basic Structure

```toml
# vx.toml - Project tool configuration

[tools]
# Version constraints
node = "22"                 # Major version (any 22.x.x)
go = "1.22"                 # Minor version (any 1.22.x)
uv = "latest"               # Always use latest
rust = "1.80"               # Specific version
just = "*"                  # Any version

# Platform-specific tools
[tools.msvc]
version = "14.42"
os = ["windows"]            # Only install on Windows

[tools.brew]
version = "latest"
os = ["macos", "linux"]

[scripts]
# Development scripts
dev = "npm run dev"
test = "cargo test"
lint = "npm run lint && cargo clippy"
build = "just build"

# CI/CD scripts
ci = "just ci"
release = "just release"

[hooks]
# Lifecycle hooks
pre_commit = ["vx run lint"]
post_setup = ["npm install", "cargo fetch"]
```

### Version Constraints

| Constraint | Example | Meaning |
|------------|---------|---------|
| Exact | `"1.2.3"` | Only version 1.2.3 |
| Major | `"1"` | Any 1.x.x |
| Minor | `"1.2"` | Any 1.2.x |
| Latest | `"latest"` | Always latest |
| Any | `"*"` | Any available version |
| Range | `">=1.0.0 <2.0.0"` | Range constraint |

### Platform-Specific Tools

```toml
[tools]
# Cross-platform tools
node = "22"
uv = "latest"

# Windows-only
[tools.msvc]
version = "14.42"
os = ["windows"]

# macOS/Linux only
[tools.brew]
version = "latest"
os = ["macos", "linux"]
```

## Project Commands

### Setup & Sync

```bash
vx setup                    # Full project setup (sync + hooks)
vx sync                     # Install all tools from vx.toml
vx sync --clean             # Remove unlisted tools
vx sync --check             # Check without installing
```

### Running Scripts

```bash
vx run dev                  # Run development server
vx run test                 # Run tests
vx run build                # Build project
vx run --list               # List available scripts
```

### Lock File

```bash
vx lock                     # Generate vx.lock
vx lock --update            # Update locked versions
vx lock --check             # Verify lock file
```

The `vx.lock` file ensures reproducible builds:

```toml
# vx.lock - Auto-generated, do not edit
[tools]
node = { version = "22.0.0", checksum = "sha256:..." }
go = { version = "1.22.0", checksum = "sha256:..." }
```

## Environment Management

### Project Environment

```bash
vx dev                      # Enter project environment
vx env list                 # List environments
vx env activate             # Print activation commands
eval $(vx env activate)     # Activate in shell
```

### Environment Variables

Define in `vx.toml`:

```toml
[env]
NODE_ENV = "development"
DATABASE_URL = "postgresql://localhost:5432/dev"
API_KEY = { env = "API_KEY", required = true }
```

## Dependency Management

### Add/Remove Tools

```bash
vx add node@22              # Add tool to vx.toml
vx add go rust uv           # Add multiple tools
vx remove node              # Remove tool from vx.toml
```

### Check Constraints

```bash
vx check                    # Verify tool constraints
vx check --json             # JSON output
vx check --fix              # Auto-fix issues
```

## Multi-Package Projects

### Monorepo Support

For monorepos, create `vx.toml` in root:

```toml
# Root vx.toml
[tools]
node = "22"
pnpm = "latest"

[scripts]
install = "pnpm install"
build = "pnpm -r build"
test = "pnpm -r test"
```

### Workspace Packages

Individual packages can have their own `vx.toml`:

```toml
# packages/backend/vx.toml
[tools]
go = "1.22"

[scripts]
dev = "go run ./cmd/server"
```

## Best Practices

### 1. Version Pinning

Pin versions for CI/CD:

```toml
[tools]
node = "22.0.0"             # Exact version for CI
```

### 2. Lock Files

Always commit `vx.lock`:

```bash
git add vx.lock
```

### 3. Scripts Organization

Group related scripts:

```toml
[scripts]
# Development
dev = "..."
watch = "..."

# Testing
test = "..."
test:watch = "..."

# Build
build = "..."
build:prod = "..."
```

### 4. Hooks for Quality

Use hooks for automated checks:

```toml
[hooks]
pre_commit = ["vx run lint", "vx run test"]
post_checkout = ["vx sync"]
```

## Project Templates

Create reusable templates:

```bash
# Create template from current project
vx template create my-template

# Use template
vx init --template my-template
```

### Template Structure

```
~/.vx/templates/my-template/
├── vx.toml
├── .gitignore
├── README.md
└── hooks/
    └── post_setup.sh
```
