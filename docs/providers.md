# Providers & Authentication

## Supported Providers

| Provider | Auth Method | Notes |
|----------|------------|-------|
| Anthropic | API Key / OAuth | Prompt caching, streaming, vision |
| OpenAI | API Key | Compatible with DeepSeek, Qwen, Ollama, vLLM |
| AWS Bedrock | SigV4 | Regional endpoints, AWS credential chain |
| Google Vertex AI | GCP OAuth2 / Service Account | Metadata server auto-detection |

---

## Custom Provider Alias

如果你的后端兼容某个内置 provider 的协议，可以给它定义一个自定义 alias，而不是把 `provider` 直接写成内置名字。

```toml
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"
```

规则：

- `provider = "my-service"` 是配置层 alias
- `[providers.my-service].provider` 必须指向底层内置 provider
- 底层 provider 目前只能是 `anthropic`、`openai`、`bedrock`、`vertex`
- alias 条目的 `model`、`api_key`、`base_url`、`compat` 会覆盖底层 provider 的默认配置

这适合 DeepSeek 网关、内部 OpenAI-compatible 服务这类场景。

---

## Profile Inheritance

Profiles support `extends` to inherit settings from another profile, avoiding duplication.

### Configuration

```toml
# Base profile
[profiles.base-anthropic]
provider = "anthropic"
api_key = "sk-ant-xxx"

# Inherits base-anthropic, overrides model
[profiles.claude-fast]
extends = "base-anthropic"
model = "claude-haiku-4-5-20251001"
max_tokens = 4096

[profiles.claude-deep]
extends = "base-anthropic"
model = "claude-opus-4-20250514"
max_tokens = 16384

# Profile can specify which MCP servers to use
[profiles.dev]
extends = "base-anthropic"
model = "claude-sonnet-4-20250514"
mcp_servers = ["filesystem", "github"]
```

### Usage

```bash
aionrs --profile claude-fast "Quick question"
aionrs --profile claude-deep "Deep security audit"
aionrs --profile dev "Create a GitHub issue"
```

- Supports multi-level inheritance chains
- Auto-detects circular inheritance
- Child profile settings override parent

---

## AWS Bedrock

Access Claude models via AWS Bedrock with SigV4 authentication.

### Configuration

```toml
[default]
provider = "bedrock"

[bedrock]
region = "us-east-1"
# Option 1: Explicit credentials
access_key_id = "AKIA..."
secret_access_key = "..."
# session_token = "..."

# Option 2: AWS profile
# profile = "my-profile"

# Option 3: Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
# Used automatically when no credentials are configured

[profiles.bedrock-claude]
provider = "bedrock"
model = "anthropic.claude-sonnet-4-20250514-v1:0"
```

### Credential Priority

1. Explicit credentials in config file
2. AWS profile
3. Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`)

---

## Google Vertex AI

Access Claude models via Google Vertex AI with GCP OAuth2 authentication.

### Configuration

```toml
[default]
provider = "vertex"

[vertex]
project_id = "my-gcp-project"
region = "us-central1"

# Option 1: Service Account key file
credentials_file = "/path/to/service-account.json"

# Option 2: Application Default Credentials
# Run: gcloud auth application-default login

# Option 3: Metadata Server (auto on GCE/GKE/Cloud Run)
# Used automatically when in GCP environments

[profiles.vertex-claude]
provider = "vertex"
model = "claude-sonnet-4@20250514"
```

### Auth Methods

| Method | Use Case |
|--------|----------|
| Service Account Key | CI/CD, server-side apps |
| Application Default Credentials | Local development (requires gcloud CLI) |
| Metadata Server | GCE/GKE/Cloud Run and other GCP environments |

---

## OAuth Login (Claude.ai)

Use your Claude.ai subscription (Pro/Team/Enterprise) directly — no API key needed.

### Login

```bash
aionrs --login
```

1. Displays an authorization URL and code
2. Open the URL in your browser and enter the code
3. Credentials are saved alongside the global config (run `aionrs --config-path` to find the directory)
4. Subsequent runs auto-load saved credentials (with auto-refresh)

### Logout

```bash
aionrs --logout
```

### Configuring OAuth Endpoints

```toml
[auth]
auth_url = "https://claude.ai/oauth"
token_url = "https://claude.ai/oauth/token"
client_id = "aionrs"
```
