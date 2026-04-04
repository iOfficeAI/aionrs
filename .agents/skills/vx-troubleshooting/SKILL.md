---
name: vx-troubleshooting
description: "Troubleshooting guide for vx issues. Use when encountering installation failures, version conflicts, PATH issues, or other vx problems."
---

# VX Troubleshooting Guide

## Common Issues

### Installation Failures

#### Tool Download Failed

**Symptoms**: `Failed to download <tool>: network error` or `Connection refused`

**Solutions**:

```bash
# Enable CDN acceleration (China users)
vx config set cdn_acceleration true

# Use mirror
vx install node --mirror https://npmmirror.com/mirrors/node

# Retry with verbose output
vx install node --verbose --debug

# Check cache and retry
vx cache clean
vx install node
```

#### Checksum Mismatch

**Symptoms**: `Checksum mismatch: expected X, got Y`

**Solutions**:

```bash
# Clear corrupted download
vx cache clean

# Reinstall with fresh download
vx install node@22 --force
```

#### Permission Denied

**Symptoms**: `Permission denied` or `Access is denied`

**Solutions**:

```bash
# Check VX_HOME permissions
ls -la ~/.vx

# Fix permissions (Unix)
chmod -R u+rw ~/.vx

# Run with elevated permissions if needed
sudo vx install node  # Not recommended, use user installation
```

### Version Issues

#### Version Not Found

**Symptoms**: `Version X not found for <tool>`

**Solutions**:

```bash
# List available versions
vx versions node

# Use latest stable
vx install node@latest

# Use LTS version
vx install node@lts

# Check for typos
vx versions node | grep "20"
```

#### Version Conflict

**Symptoms**: Multiple versions installed, wrong version active

**Solutions**:

```bash
# List installed versions
vx list node --installed

# Switch to specific version
vx switch node@20

# Check which version is active
vx which node

# Remove conflicting versions
vx uninstall node@18
```

### PATH Issues

#### Command Not Found

**Symptoms**: `command not found: node` after installation

**Solutions**:

```bash
# Verify vx shim directory is in PATH
echo $PATH | grep -o ".vx/bin"

# Add vx to PATH (add to shell config)
export PATH="$HOME/.vx/bin:$PATH"

# Or use vx directly
vx node --version

# Check shim exists
ls ~/.vx/bin/node
```

#### Wrong Version in PATH

**Symptoms**: System version takes precedence over vx version

**Solutions**:

```bash
# Check which executable is being used
which node
vx which node

# Reorder PATH (vx should come first)
export PATH="$HOME/.vx/bin:$PATH"

# Or use vx explicitly
vx node --version
```

### Runtime Issues

#### Tool Crashes on Startup

**Symptoms**: Tool exits immediately or crashes

**Solutions**:

```bash
# Check tool version
vx which node
vx node --version

# Reinstall the tool
vx install node --force

# Check for missing dependencies
vx doctor

# Try with debug output
vx node --verbose script.js
```

#### Dependency Missing

**Symptoms**: `error while loading shared libraries` or `DLL not found`

**Solutions**:

```bash
# Check dependencies (Linux)
ldd $(vx which node)

# Install system dependencies
# Ubuntu/Debian
sudo apt-get install build-essential libssl-dev

# macOS (via Homebrew)
brew install openssl

# Windows - usually bundled, check PATH
```

### Configuration Issues

#### vx.toml Not Loading

**Symptoms**: Settings in vx.toml ignored

**Solutions**:

```bash
# Verify file location
ls vx.toml

# Check syntax
vx check

# Validate configuration
vx config validate

# Show effective configuration
vx config show
```

#### Lock File Conflicts

**Symptoms**: `vx.lock is out of sync`

**Solutions**:

```bash
# Regenerate lock file
vx lock --update

# Or remove and regenerate
rm vx.lock
vx lock
```

## Diagnostic Commands

### System Information

```bash
# General diagnostics
vx doctor

# System info
vx info

# Environment check
vx check --json

# Show configuration
vx config show
```

### Debug Mode

```bash
# Enable debug logging
vx --debug node --version

# Enable trace logging
vx --trace node --version

# Verbose output
vx --verbose install node
```

### Cache Inspection

```bash
# Show cache location
vx cache dir

# Show cache size
vx cache info

# List cached items
vx cache list

# Clean cache
vx cache clean
```

## Error Messages Reference

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `Tool not found` | Unknown tool name | Check `vx list` for available tools |
| `Version not found` | Invalid version | Use `vx versions <tool>` to see available |
| `Network error` | Connection issues | Check network, enable CDN, use mirror |
| `Permission denied` | Insufficient permissions | Check directory permissions |
| `Checksum mismatch` | Corrupted download | Run `vx cache clean` and retry |
| `Out of disk space` | Disk full | Clean cache: `vx cache clean` |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Tool not found |
| 3 | Installation failed |
| 4 | Version not found |
| 5 | Network error |
| 6 | Permission error |
| 7 | Configuration error |

## Recovery Procedures

### Complete Reset

```bash
# Backup configuration
cp -r ~/.vx ~/.vx.backup

# Remove everything
rm -rf ~/.vx

# Reinstall
vx install node go uv

# Restore configuration
cp ~/.vx.backup/vx.toml ~/.vx/
```

### Repair Installation

```bash
# Verify and repair
vx doctor --fix

# Reinstall all tools from vx.toml
vx sync --force

# Rebuild shims
vx shim rebuild
```

## Getting Help

### Collect Diagnostics

```bash
# Generate diagnostic report
vx doctor --output diagnostics.txt

# Include in bug report
cat diagnostics.txt
```

### Useful Information to Provide

1. vx version: `vx --version`
2. Operating system: `vx info | grep -i os`
3. Command that failed
4. Error message (use `--debug`)
5. Contents of `vx.toml` (if applicable)
6. `vx doctor` output

### Support Channels

- GitHub Issues: https://github.com/loonghao/vx/issues
- Documentation: https://github.com/loonghao/vx#readme

## Quick Triage for AI Agents

When a user reports a vx issue, follow this decision tree:

```
1. "command not found: vx"
   → vx is not installed. Run the install script.
   → Linux/macOS: curl -fsSL https://raw.githubusercontent.com/loonghao/vx/main/install.sh | bash
   → Windows: powershell -c "irm https://raw.githubusercontent.com/loonghao/vx/main/install.ps1 | iex"

2. "Failed to download" / "network error" (exit code 5)
   → Try: vx cache clean && vx install <tool> --verbose
   → If in China: vx config set cdn_acceleration true
   → Check if GITHUB_TOKEN is set for API rate limits

3. "version not found" (exit code 4)
   → Run: vx versions <tool> to list available versions
   → The user may have a typo in the version string
   → Try: vx install <tool>@latest

4. "permission denied" (exit code 6)
   → Check: ls -la ~/.vx (Unix) or icacls %USERPROFILE%\.vx (Windows)
   → Fix: chmod -R u+rw ~/.vx
   → Never use sudo with vx

5. Tool works but wrong version
   → Run: vx which <tool> to see which version is active
   → Check: vx.toml may specify a different version
   → Run: vx switch <tool>@<version>

6. vx.toml not being picked up (exit code 7)
   → Ensure file is in the project root (same dir as .git)
   → Run: vx check to validate syntax

7. CI failing with vx
   → Ensure the GitHub Action is used: loonghao/vx@main
   → Add github-token for rate limit avoidance
   → Use cache: 'true' for faster CI runs

8. General error (exit code 1)
   → Run: vx doctor for full diagnostics
   → Run: vx --debug <command> for detailed logs
   → Check: vx cache clean to clear corrupted state
```
