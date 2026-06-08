# Troubleshooting

## API Key Not Configured

```
No API key found. Provide via --api-key, config file, or environment variable
```

Provide an API key via any of: config file, `--api-key` flag, or environment variable.

## Invalid API Key

```
[error] API error: API error 401: ...
```

Verify your API key is correct and active.

## Profile Not Found

```
Profile 'xxx' not found in config
```

Check that the profile is defined in your config file.

## Model Not Available

```
[error] API error: API error 404: ...
```

Check that `--model` is spelled correctly and your API key has access to that model.

## Request Too Large

```
[error] API error: API error 413: ...
```

Conversation history is too long. Restart the agent or reduce `--max-turns`.

## Rate Limited

```
[error] Provider error: Rate limited, retry after 5000ms
```

API call frequency is too high. The agent will auto-retry after the indicated delay.

## Command Timeout

```
Command timed out after 120000ms
```

A ExecCommand tool command exceeded the timeout. Increase the timeout via the tool's `timeout` parameter.

## ripgrep Not Installed

The Grep tool automatically falls back to system `grep`. For better search performance:

```bash
brew install ripgrep  # macOS
sudo apt install ripgrep  # Debian/Ubuntu
```
