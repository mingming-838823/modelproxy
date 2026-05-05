# ModelProxy

A high-performance, self-hosted AI model gateway and proxy server built with Rust. Unify access to multiple LLM providers (OpenAI, Anthropic, Ollama, Minimax, and more) through a single OpenAI-compatible API endpoint.

[中文文档](README_zh.md)

## Features

- **Multi-Provider Proxy** — Forward requests to OpenAI, Anthropic, Ollama, Minimax, and any OpenAI-compatible API through a single endpoint
- **Smart Routing** — Route requests to different models based on conditions: token count, keywords, image presence, or time ranges
- **Automatic Format Conversion** — Transparently convert between OpenAI, Anthropic, and Ollama request/response formats
- **Streaming Support** — Full SSE streaming with real-time format conversion for all providers
- **Multi-Tenancy** — Tenant isolation with per-tenant upstream configurations and user management
- **API Key Management** — Generate and manage API keys with fine-grained rate limits (RPM/TPM/daily)
- **Model Visibility Control** — Control which models are visible to which users
- **Retry & Fallback** — Configurable retry strategies (fixed, exponential, exponential-jitter) with fallback routing on failure
- **Upstream Load Balancing** — Priority-based, weighted, and round-robin balancing across upstreams
- **Rate Limiting** — Per-key and per-upstream rate limiting with automatic upstream blocking on 429 errors
- **Usage Tracking & Billing** — Token usage tracking, cost calculation with configurable pricing per model
- **Audit Logging** — Comprehensive request/response logging for compliance and debugging
- **Web Admin Dashboard** — Built-in responsive admin UI with i18n (Chinese/English)
- **Encrypted Storage** — Upstream API keys are encrypted at rest with AES-256-GCM
- **SQLite Backend** — Zero-dependency database, easy to deploy and backup
- **GUI Mode** — Optional system tray application for desktop use

## Use Cases

ModelProxy is an LLM intelligent routing proxy written in Rust, designed for the following scenarios:

**Enterprise LLM Distribution** — Ideal for SMEs to centrally manage and distribute LLM capabilities, providing stable and controllable AI services to internal teams through a single entry point.

**Personal Upstream Consolidation** — Helps individual developers consolidate multiple upstream LLM resources (including free services and paid Coding Plans), solving issues like service instability and frequent rate limiting.

### Core Capabilities

- **Multi-Source Failover** — Configure multiple upstream LLM services as mutual backups. When one service is rate-limited or unavailable, the proxy automatically switches to the next available service. The failover is completely transparent to clients, significantly improving access stability.
- **Unified Aliases, Seamless Upgrades** — After upstream model upgrades, use aliases to serve clients under a consistent name. Clients enjoy the latest model capabilities without any configuration changes.
- **Smart Routing, On-Demand Distribution** — Route requests intelligently based on time ranges, estimated token counts, request keywords, and more. For example: route low-token requests to a locally deployed LLM server to save costs, and high-token requests to cloud LLMs for greater compute power.
- **Multimodal Capability Extension** — When the primary model is text-only (e.g., DeepSeek V4), the proxy can intelligently detect whether client requests contain image content and automatically forward them to a multimodal model (e.g., Qwen 3.6), effectively giving text-only models multimodal processing capabilities.

## Architecture

```
┌─────────────┐     ┌──────────────────────────────────────────┐     ┌──────────────┐
│   Client     │────▶│              ModelProxy                  │────▶│  OpenAI API  │
│  (any SDK)   │     │                                          │     ├──────────────┤
└─────────────┘     │  ┌─────────┐  ┌──────────┐  ┌────────┐ │     │ Anthropic API│
                    │  │  Auth   │  │  Smart   │  │ Retry  │ │     ├──────────────┤
                    │  │  & Rate │  │  Routing │  │ Logic  │ │     │  Ollama API  │
                    │  │  Limit  │  │          │  │        │ │     ├──────────────┤
                    │  └─────────┘  └──────────┘  └────────┘ │     │  Minimax API │
                    │                                          │     ├──────────────┤
                    │  ┌─────────────────────────────────────┐ │     │  Custom API  │
                    │  │     Format Conversion Engine         │ │     └──────────────┘
                    │  │  OpenAI ↔ Anthropic ↔ Ollama        │ │
                    │  └─────────────────────────────────────┘ │
                    │                                          │
                    │  ┌──────────┐  ┌──────────┐  ┌────────┐ │
                    │  │  SQLite  │  │  Memory  │  │  Audit │ │
                    │  │ Database │  │  Cache   │  │  Log   │ │
                    │  └──────────┘  └──────────┘  └────────┘ │
                    └──────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.75+ (for building from source)
- SQLite3 (bundled with the Rust `sqlx` crate)

### Build

```bash
cargo build --release
```

The binary will be at `target/release/modelproxy`.

### Initialize

```bash
./modelproxy init
```

This launches an interactive setup wizard that configures:
- Database path
- Proxy server port (default: 3000)
- Admin dashboard port (default: 3001)
- Admin password

### Run

```bash
./modelproxy
```

Two services will start:
- **Proxy API** at `http://127.0.0.1:3000` — OpenAI-compatible endpoint for clients
- **Admin Dashboard** at `http://127.0.0.1:3001` — Web UI for management

### GUI Mode

```bash
cargo run --bin modelproxy-gui --release
```

Launches a system tray application with an embedded web view.

## Configuration

Configuration is stored in `config.json` (auto-generated on first run):

```json
{
  "server": {
    "host": "127.0.0.1",
    "port": 3000,
    "workers": 8
  },
  "admin": {
    "host": "127.0.0.1",
    "port": 3001,
    "base_url": null,
    "allow_public_registration": false
  },
  "database": {
    "path": "data/modelproxy.db",
    "max_connections": 10
  },
  "jwt": {
    "secret": "<auto-generated>",
    "expiration_hours": 0
  },
  "proxy": {
    "request_timeout_secs": 300,
    "connect_timeout_secs": 30,
    "max_idle_connections": 200,
    "max_request_body_bytes": 33554432,
    "max_text_request_body_bytes": 2097152,
    "max_multimodal_request_body_bytes": 20971520
  },
  "rate_limit": {
    "cleanup_interval_secs": 60,
    "window_size_secs": 60
  }
}
```

| Field | Description |
|-------|-------------|
| `server.host/port` | Proxy API listen address |
| `admin.host/port` | Admin dashboard listen address |
| `admin.allow_public_registration` | Allow anyone to register an account |
| `database.path` | SQLite database file path |
| `jwt.secret` | JWT signing key (auto-generated if insecure) |
| `proxy.request_timeout_secs` | Upstream request timeout |
| `proxy.max_request_body_bytes` | Hard limit for request body size |
| `proxy.max_text_request_body_bytes` | Soft limit for text-only requests |
| `proxy.max_multimodal_request_body_bytes` | Soft limit for multimodal requests |

## API Endpoints

### Proxy API (Port 3000)

OpenAI-compatible endpoints:

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/models` | List available models |
| POST | `/v1/chat/completions` | Chat completion (OpenAI format) |
| POST | `/v1/completions` | Text completion |
| POST | `/v1/messages` | Chat completion (Anthropic format passthrough) |

Authentication: `Authorization: Bearer <your-api-key>`

### Admin API (Port 3001)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/auth/login` | Login |
| GET | `/api/upstreams` | List upstreams |
| POST | `/api/upstreams` | Create upstream |
| PUT | `/api/upstreams/:id` | Update upstream |
| DELETE | `/api/upstreams/:id` | Delete upstream |
| POST | `/api/upstreams/:id/test` | Test upstream connectivity |
| GET | `/api/models` | List models with visibility |
| PUT | `/api/models/:upstream_id/:model_name` | Set model visibility |
| GET | `/api/models/conditional-aliases` | List smart routes |
| PUT | `/api/models/conditional-aliases/:alias` | Create/update smart route |
| DELETE | `/api/models/conditional-aliases/:alias` | Delete smart route |
| GET | `/api/users` | List users |
| POST | `/api/users` | Create user |
| GET | `/api/keys` | List API keys |
| POST | `/api/keys` | Create API key |
| GET | `/api/usage/me` | Get my usage |
| GET | `/api/conversations/me` | Get my conversations |
| GET | `/api/audit/proxy` | List proxy audit logs |
| GET | `/api/pricing` | List pricing configs |
| PUT | `/api/pricing/:model_key` | Set model pricing |

## Smart Routing

Smart Routing allows you to route requests to different upstream models based on conditions. When a client requests a model alias, the proxy evaluates rules in priority order and forwards to the first matching model.

### Supported Conditions

| Condition | Description |
|-----------|-------------|
| `token_gt` | Input tokens exceed a threshold |
| `token_lt` | Input tokens are below a threshold |
| `keyword` | Request text contains specified keywords |
| `has_image` | Request contains image content (multimodal) |
| `time_range` | Current time falls within a specified range |

### Example

Create a smart route alias `smart-chat`:
- If input tokens > 4000 → route to `gpt-4-32k`
- If request contains image → route to `gpt-4-vision-preview`
- If keyword contains "code" → route to `codellama`
- Fallback → route to `gpt-3.5-turbo`

Clients simply use `model: "smart-chat"` in their requests, and the proxy handles the rest.

## Retry & Fallback

Each model can be configured with retry settings:

| Setting | Description |
|---------|-------------|
| `retry_count` | Number of retry attempts (0 = no retry) |
| `retry_interval_seconds` | Base delay between retries |
| `retry_backoff_strategy` | `fixed`, `exponential`, or `exponential_jitter` |
| `retry_max_interval_seconds` | Maximum delay cap |
| `retry_failure_strategy` | `error` (return error) or `route` (fallback to another model) |
| `retry_fallback_upstream_id` | Fallback upstream when strategy is `route` |
| `retry_fallback_model_name` | Fallback model name |

## Usage with OpenAI SDK

```python
from openai import OpenAI

client = OpenAI(
    api_key="your-modelproxy-api-key",
    base_url="http://127.0.0.1:3000/v1"
)

response = client.chat.completions.create(
    model="smart-chat",
    messages=[{"role": "user", "content": "Hello!"}],
    stream=True
)

for chunk in response:
    print(chunk.choices[0].delta.content, end="")
```

## Maintenance Commands

```bash
# Migrate plaintext API keys to encrypted storage
./modelproxy migrate-upstream-secrets

# Rotate encryption keys (re-encrypts all secrets)
./modelproxy rotate-secrets
```

## Tech Stack

- **Backend**: Rust, Axum, SQLx, Reqwest, Tokio
- **Frontend**: Vanilla JavaScript, CSS (embedded in binary)
- **Database**: SQLite
- **Encryption**: AES-256-GCM for API key storage
- **GUI**: Iced (optional)

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
