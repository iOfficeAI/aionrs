---
name: vx-commands
description: "Complete vx CLI command reference. Use when looking up specific vx command syntax, flags, or output formats. All commands support --json for structured output and --format toon for token-optimized output."
---

# VX Command Reference

## Structured Output Commands (AI-Optimized)

All commands support `--json` for structured output and `--format toon` for token-optimized output (saves 40-60% tokens).

### Project Analysis

```bash
vx analyze --json           # Analyze project structure
vx check --json             # Verify tool constraints
vx ai context --json        # Generate AI-friendly project context
```

**Output fields (analyze)**:
- `ecosystems[]` - Detected ecosystems (nodejs, python, rust, go)
- `dependencies[]` - Project dependencies
- `scripts[]` - Available scripts
- `required_tools[]` - Tools needed

**Output fields (check)**:
- `requirements[]` - Tool requirement status
- `all_satisfied` - Whether all constraints are met
- `missing_tools[]` - Tools that need installation

**Output fields (ai context)**:
- `project` - Project info (name, languages, frameworks)
- `tools[]` - Installed tools with versions
- `scripts[]` - Available scripts
- `constraints[]` - Tool constraints

### Tool Management

```bash
vx list --json              # List installed tools
vx versions node --json     # List available versions
vx which node --json        # Find tool location
vx search <query> --json    # Search for tools
```

**Output fields (list)**:
- `runtimes[]` - Available runtimes
- `total` - Total count
- `installed_count` - Installed count

**Output fields (versions)**:
- `versions[]` - Available versions with metadata
- `latest` - Latest version
- `lts` - LTS version (if applicable)

**Output fields (which)**:
- `path` - Executable path
- `version` - Resolved version
- `source` - Source (vx, system, global_package)

### Installation

```bash
vx install node@22 --json   # Install tool
vx sync --json              # Sync from vx.toml
```

**Output fields (install)**:
- `runtime` - Tool name
- `version` - Installed version
- `path` - Installation path
- `duration_ms` - Installation duration
- `dependencies_installed[]` - Dependencies also installed

**Output fields (sync)**:
- `installed[]` - Successfully installed
- `skipped[]` - Skipped (already installed)
- `failed[]` - Failed installations
- `duration_ms` - Total duration

### AI Integration

```bash
vx ai context               # Generate AI-friendly context (Markdown)
vx ai context --json        # JSON format
vx ai context --minimal     # Minimal output
vx ai session init          # Initialize session state
vx ai session status        # Show session status
vx ai session cleanup       # Clean up session
```

### Environment

```bash
vx env --json               # Show environment variables
vx dev --export             # Export shell environment
```

## Output Formats

### JSON Format
```bash
vx list --json
# Output: {"runtimes": [...], "total": 50, "installed_count": 5}
```

### TOON Format (Token-Optimized)
```bash
vx list --format toon
# Output:
# runtimes[50]{name,installed,description}:
#   node,true,Node.js runtime
#   python,false,Python runtime
#   ...
```

TOON format is recommended for AI agents - it saves 40-60% tokens compared to JSON.

### Environment Variable
```bash
export VX_OUTPUT=json       # Default to JSON output
export VX_OUTPUT=toon       # Default to TOON output
```

## Command Groups

### Tool Execution
```bash
vx <tool> [args...]         # Run any tool
vx npm install              # Run npm
vx cargo build              # Run cargo
```

### Advanced Execution Syntax
```bash
# Runtime with version
vx node@22 app.js                     # Use specific Node.js version

# Runtime executable override
vx msvc@14.42::cl main.cpp            # Run cl from MSVC runtime

# Package execution (ecosystem:package pattern)
vx npm:vite                            # Run vite via npm
vx uv:ruff check .                    # Run ruff via uv
vx npm:typescript@5.0::tsc            # Run tsc from specific typescript version

# Package aliases (shortcuts)
vx vite                                # Same as: vx npm:vite
vx meson                              # Same as: vx uv:meson

# Multi-runtime composition
vx --with bun@1.1 --with deno node app.js   # Multiple runtimes in PATH
```

### Tool Management
```bash
vx install <tool>@<version> # Install tool
vx uninstall <tool>         # Uninstall tool
vx list                     # List tools
vx versions <tool>          # Show versions
vx which <tool>             # Find tool path
vx switch <tool>@<version>  # Switch version
```

### Project Management
```bash
vx init                     # Initialize vx.toml
vx add <tool>               # Add tool to project
vx remove <tool>            # Remove tool from project
vx sync                     # Sync tools from vx.toml
vx lock                     # Generate vx.lock
vx check                    # Check constraints
```

### Script Execution
```bash
vx run <script>             # Run script from vx.toml
vx run --list               # List available scripts
```

### Cache Management
```bash
vx cache dir                # Show cache directory
vx cache clean              # Clean cache
```

## Global Flags

```bash
--json                      # JSON output
--format <text|json|toon>   # Output format
--verbose                   # Verbose output
--debug                     # Debug output
--use-system-path           # Use system PATH
--cache-mode <mode>         # Cache mode (normal, refresh, offline, no-cache)
```

## Exit Codes

- `0` - Success
- `1` - General error
- `2` - Tool not found
- `3` - Installation failed
- `4` - Version not found
- `5` - Network error
- `6` - Permission error
- `7` - Configuration error
