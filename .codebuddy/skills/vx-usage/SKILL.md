---
name: vx-usage
description: "Teaches AI agents how to use vx, the universal dev tool manager. Use when the project has vx.toml or .vx/, or when the user mentions vx, tool version management, or cross-platform setup. vx auto-manages Node.js, Python, Go, Rust, and 78 tools via Starlark DSL providers."
---

# VX - Universal Development Tool Manager

vx is a universal development tool manager that automatically installs and manages
development tools (Node.js, Python/uv, Go, Rust, etc.) with zero configuration.

## Core Concept

Instead of requiring users to manually install tools, prefix any command with `vx`:

```bash
vx node --version      # Auto-installs Node.js if needed
vx uv pip install x    # Auto-installs uv if needed
vx go build .          # Auto-installs Go if needed
vx cargo build         # Auto-installs Rust if needed
vx just test           # Auto-installs just if needed
```

vx is fully transparent - same commands, same arguments, just add `vx` prefix.

## Essential Commands

### Tool Execution (most common)
```bash
vx <tool> [args...]           # Run any tool (auto-installs if missing)
vx node app.js                # Run Node.js
vx python script.py           # Run Python (via uv)
vx npm install                # Run npm
vx npx create-react-app app   # Run npx
vx cargo test                 # Run cargo
vx just build                 # Run just (task runner)
vx git status                 # Run git
```

### Tool Management
```bash
vx install node@22            # Install specific version
vx install uv go rust         # Install multiple tools at once
vx list                       # List all available tools
vx list --installed           # List installed tools only
vx versions node              # Show available versions
vx switch node@20             # Switch active version
vx uninstall go@1.21          # Remove a version
```

### Project Management
```bash
vx init                       # Initialize vx.toml for project
vx sync                       # Install all tools from vx.toml
vx setup                      # Full project setup (sync + hooks)
vx dev                        # Enter dev environment with all tools
vx run test                   # Run project scripts from vx.toml
vx check                      # Verify tool constraints
vx lock                       # Generate vx.lock for reproducibility
```

### Environment & Config
```bash
vx env list                   # List environments
vx config show                # Show configuration
vx cache info                 # Show cache usage
vx search <query>             # Search available tools
vx info                       # System info and capabilities
```

## Project Configuration (vx.toml)

Projects use `vx.toml` in the root directory:

```toml
[tools]
node = "22"         # Major version
go = "1.22"         # Minor version
uv = "latest"       # Always latest
rust = "1.80"       # Specific version
just = "*"          # Any version

[scripts]
dev = "npm run dev"
test = "cargo test"
lint = "npm run lint && cargo clippy"
build = "just build"

[hooks]
pre_commit = ["vx run lint"]
post_setup = ["npm install"]
```

## Using `--with` for Multi-Runtime

When a command needs additional runtimes available:

```bash
vx --with bun node app.js     # Node.js + Bun in PATH
vx --with deno npm test        # npm + Deno available
```

## Package Aliases

vx supports **package aliases** — short commands that automatically route to ecosystem packages:

```bash
# These are equivalent:
vx vite              # Same as: vx npm:vite
vx vite@5.0          # Same as: vx npm:vite@5.0
vx rez               # Same as: vx uv:rez
vx pre-commit        # Same as: vx uv:pre-commit
vx meson             # Same as: vx uv:meson
vx release-please    # Same as: vx npm:release-please
```

**Benefits**:
- Simpler commands without remembering ecosystem prefixes
- Automatic runtime dependency management (node/python installed as needed)
- Respects project `vx.toml` version configuration

**Available Aliases**:
| Short Command | Equivalent | Ecosystem |
|--------------|------------|-----------|
| `vx vite` | `vx npm:vite` | npm |
| `vx release-please` | `vx npm:release-please` | npm |
| `vx rez` | `vx uv:rez` | uv |
| `vx pre-commit` | `vx uv:pre-commit` | uv |
| `vx meson` | `vx uv:meson` | uv |

## Companion Tool Environment Injection

When `vx.toml` includes tools like MSVC, vx automatically injects discovery environment variables into **all** subprocess environments. This allows any tool needing a C/C++ compiler to discover the vx-managed installation.

```toml
# vx.toml — MSVC env vars injected for ALL tools
[tools]
node = "22"
cmake = "3.28"
rust = "1.82"

[tools.msvc]
version = "14.42"
os = ["windows"]
```

Now tools like node-gyp, CMake, Cargo (cc crate) automatically find MSVC:

```bash
# node-gyp finds MSVC via VCINSTALLDIR
vx npx node-gyp rebuild

# CMake discovers the compiler
vx cmake -B build -G "Ninja"

# Cargo cc crate finds MSVC for C dependencies
vx cargo build
```

**Injected Environment Variables** (MSVC example):
| Variable | Purpose |
|----------|---------|
| `VCINSTALLDIR` | VS install path (node-gyp, CMake) |
| `VCToolsInstallDir` | Exact toolchain path |
| `VX_MSVC_ROOT` | vx MSVC root path |

## MSVC Build Tools (Windows)

Microsoft Visual C++ compiler for Windows development:

```bash
# Install MSVC Build Tools
vx install msvc@latest
vx install msvc 14.40       # Specific version

# Using MSVC tools via namespace
vx msvc cl main.cpp -o main.exe
vx msvc link main.obj
vx msvc nmake

# Direct aliases
vx cl main.cpp              # Same as: vx msvc cl
vx nmake                    # Same as: vx msvc nmake

# Version-specific usage
vx msvc@14.40 cl main.cpp
```

**Available MSVC Tools**:
| Tool | Command | Description |
|------|---------|-------------|
| cl | `vx msvc cl` | C/C++ compiler |
| link | `vx msvc link` | Linker |
| lib | `vx msvc lib` | Library manager |
| nmake | `vx msvc nmake` | Make utility |

## Supported Tools (78 Providers)

| Category | Tools |
|----------|-------|
| **JavaScript** | node, npm, npx, bun, deno, pnpm, yarn, vite, nx, turbo |
| **JS Tooling** | oxlint |
| **Python** | uv, uvx, python, pip, ruff, maturin, pre-commit |
| **Rust** | cargo, rustc, rustup |
| **Go** | go, gofmt, gws |
| **System/CLI** | git, bash, curl, pwsh, jq, yq, fd, bat, ripgrep, fzf, starship, jj |
| **Build Tools** | just, task, cmake, ninja, make, meson, xmake, protoc, conan, vcpkg, spack |
| **DevOps** | kubectl, helm, podman, terraform, hadolint, dagu |
| **Cloud CLI** | awscli, azcli, gcloud |
| **.NET** | dotnet, msbuild, nuget |
| **C/C++** | msvc, llvm, nasm, ccache, buildcache, sccache, rcedit |
| **Media** | ffmpeg, imagemagick |
| **Java** | java |
| **AI** | ollama, openclaw |
| **Other Langs** | zig |
| **Package Managers** | brew, choco, winget |
| **Misc** | gh, prek, actrun, wix, vscode, xcodebuild, systemctl, release-please, rez, 7zip |

## Provider System (Starlark DSL)

All 78 providers are defined using **provider.star** (Starlark DSL) — a declarative, zero-compilation approach. Each provider lives in `crates/vx-providers/<name>/provider.star`.

vx uses a **two-phase execution model** (inspired by Buck2):
1. **Analysis Phase (Starlark)**: `provider.star` runs as pure computation, returning descriptor dicts. No I/O.
2. **Execution Phase (Rust)**: The Rust runtime interprets descriptors for actual downloads, installs, and process execution.

### How to add a new tool

```starlark
# crates/vx-providers/mytool/provider.star
load("@vx//stdlib:provider.star", "runtime_def", "github_permissions")
load("@vx//stdlib:provider_templates.star", "github_rust_provider")

name        = "mytool"
description = "My awesome tool"
ecosystem   = "custom"

runtimes = [runtime_def("mytool", aliases=["mt"])]
permissions = github_permissions()

# Use a template — covers 90% of tools
_p = github_rust_provider("owner", "mytool",
    asset = "mytool-{vversion}-{triple}.{ext}")
fetch_versions   = _p["fetch_versions"]
download_url     = _p["download_url"]
install_layout   = _p["install_layout"]
store_root       = _p["store_root"]
get_execute_path = _p["get_execute_path"]
environment      = _p["environment"]
```

### Available templates

| Template | Use case | Example |
|----------|----------|---------|
| `github_rust_provider` | Rust tools on GitHub (most common) | ripgrep, fd, bat, just, uv |
| `github_go_provider` | Go tools on GitHub (goreleaser style) | gh, task |
| `github_binary_provider` | Single binary download (no archive) | kubectl |
| `system_provider` | System package manager only | 7zip |

### Template Placeholders

| Placeholder | Rust template | Go template | Description |
|-------------|---------------|-------------|-------------|
| `{version}` | ✓ | ✓ | Version number (e.g., "1.0.0") |
| `{vversion}` | ✓ | — | With v-prefix (e.g., "v1.0.0") |
| `{triple}` | ✓ | — | Rust target triple (e.g., "x86_64-unknown-linux-musl") |
| `{os}` | — | ✓ | Go GOOS (linux, darwin, windows) |
| `{arch}` | — | ✓ | Go GOARCH (amd64, arm64) |
| `{ext}` | ✓ | ✓ | Archive extension (zip/tar.gz) |
| `{exe}` | ✓ | ✓ | Executable suffix (.exe/"") |

### Starlark Standard Library (14 modules)

Located in `crates/vx-starlark/stdlib/`. The main entry point is `provider.star` which re-exports everything:

```starlark
# Import everything from the unified facade
load("@vx//stdlib:provider.star",
     "runtime_def", "bundled_runtime_def", "dep_def",
     "github_permissions", "platform_map",
     "env_set", "env_prepend",
     "archive_layout", "binary_layout")
```

Key modules:
- `provider.star` — Unified facade (re-exports all)
- `runtime.star` — `runtime_def`, `bundled_runtime_def`, `dep_def`
- `platform.star` — `platform_map`, `platform_select`, `rust_triple`, `go_os_arch`
- `env.star` — `env_set`, `env_prepend`, `env_append`, `env_unset`
- `layout.star` — `archive_layout`, `binary_layout`, `bin_subdir_layout`, hooks
- `provider_templates.star` — High-level templates (4 templates)
- `permissions.star` — `github_permissions`, `system_permissions`
- `system_install.star` — `winget_install`, `brew_install`, `apt_install`

## Important Rules for AI Agents

1. **Always use `vx` prefix** when running tools in vx-managed projects
2. **Use `vx just`** instead of `just` for task runner commands
3. **Use `vx npm`** instead of `npm` directly
4. **Use `vx cargo`** instead of `cargo` directly
5. **Check `vx.toml`** first to understand project tool requirements
6. **Use `vx run <script>`** for project-defined scripts
7. **Never suggest manual tool installation** - vx handles it automatically
8. Commands like `vx install` are for explicit pre-installation; normal usage auto-installs
9. **Use correct terminology**: Runtime (not Tool), Provider (not Plugin), provider.star (not provider config)
10. **Provider development**: New tools are added via `provider.star` Starlark DSL in `crates/vx-providers/<name>/`
11. **Tests go in `tests/` dirs** — never inline `#[cfg(test)]` in source files

## GitHub Actions Integration

vx provides a GitHub Action (`action.yml`) for CI/CD workflows. Use it in `.github/workflows/` files:

### Basic Usage

```yaml
- uses: loonghao/vx@main
  with:
    version: 'latest'           # vx version (default: latest)
    github-token: ${{ secrets.GITHUB_TOKEN }}
```

### Pre-install Tools

```yaml
- uses: loonghao/vx@main
  with:
    tools: 'node go uv'         # Space-separated tools to pre-install
    cache: 'true'               # Enable tool caching (default: true)
```

### Project Setup (vx.toml)

```yaml
- uses: loonghao/vx@main
  with:
    setup: 'true'               # Run `vx setup --ci` for vx.toml projects
```

### Full Example

```yaml
name: CI
on: [push, pull_request]

jobs:
  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]

    steps:
      - uses: actions/checkout@v6

      - uses: loonghao/vx@main
        with:
          tools: 'node@22 uv'
          setup: 'true'
          cache: 'true'

      - run: vx node --version
      - run: vx npm test
```

### Action Inputs

| Input | Default | Description |
|-------|---------|-------------|
| `version` | `latest` | vx version to install |
| `github-token` | `${{ github.token }}` | GitHub token for API requests |
| `tools` | `''` | Space-separated tools to pre-install |
| `cache` | `true` | Enable caching of ~/.vx directory |
| `cache-key-prefix` | `vx-tools` | Custom prefix for cache key |
| `setup` | `false` | Run `vx setup --ci` for vx.toml projects |

### Action Outputs

| Output | Description |
|--------|-------------|
| `version` | The installed vx version |
| `cache-hit` | Whether the cache was hit |

## Container Image Support

vx also provides a container image for containerized workflows, and it can be consumed from Podman-compatible environments:


```dockerfile
# Use vx as base image
FROM ghcr.io/loonghao/vx:latest

# Tools are auto-installed on first use
RUN vx node --version
RUN vx uv pip install mypackage
```

### Multi-stage Build with vx

```dockerfile
FROM ghcr.io/loonghao/vx:latest AS builder
RUN vx node --version && vx npm ci && vx npm run build

FROM nginx:alpine
COPY --from=builder /home/vx/dist /usr/share/nginx/html
```

### GitHub Actions Container Jobs

```yaml
jobs:
  build:
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/loonghao/vx:latest
    steps:
      - uses: actions/checkout@v6
      - run: vx node --version
      - run: vx npm test
```
