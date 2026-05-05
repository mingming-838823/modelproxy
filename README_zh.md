# ModelProxy

基于 Rust 构建的高性能、自托管 AI 模型网关与代理服务器。通过统一的 OpenAI 兼容 API 端点，聚合访问多个大模型服务商（OpenAI、Anthropic、Ollama、Minimax 等）。

[English](README.md)

## 功能特性

- **多服务商代理** — 通过单一端点将请求转发至 OpenAI、Anthropic、Ollama、Minimax 及任何 OpenAI 兼容 API
- **智能路由** — 根据条件将请求路由到不同模型：Token 数量、关键词、是否包含图片、时间段
- **自动格式转换** — 透明地在 OpenAI、Anthropic、Ollama 请求/响应格式之间转换
- **流式支持** — 完整的 SSE 流式传输，支持所有服务商的实时格式转换
- **多租户** — 租户隔离，每个租户独立的上游配置和用户管理
- **API Key 管理** — 生成和管理 API Key，支持精细的速率限制（RPM/TPM/每日限额）
- **模型可见性控制** — 控制不同用户可访问的模型
- **重试与降级** — 可配置的重试策略（固定、指数退避、指数抖动），失败后自动降级路由
- **上游负载均衡** — 基于优先级、权重和轮询的上游负载均衡
- **速率限制** — 按 Key 和按上游的速率限制，429 错误时自动屏蔽上游
- **用量追踪与计费** — Token 用量追踪，按模型配置价格计算成本
- **审计日志** — 完整的请求/响应日志记录，满足合规和调试需求
- **Web 管理后台** — 内置响应式管理界面，支持中英文切换
- **加密存储** — 上游 API Key 使用 AES-256-GCM 加密存储
- **SQLite 后端** — 零依赖数据库，部署和备份简单
- **GUI 模式** — 可选的系统托盘应用，适用于桌面使用

## 适用场景

ModelProxy 是一个由 Rust 编写的 LLM 智能路由代理，适用于以下场景：

**企业级大模型能力分发** — 适合中小企业统一管理和分发大模型能力，通过单一入口为内部团队提供稳定、可控的 AI 服务。
**个人上游资源整合** — 帮助个人开发者整合多个上游 LLM 资源（包括免费服务和付费 Coding Plan），解决单一服务不稳定、频繁限流等问题。

### 核心能力

- **多源互备，无缝切换** — 将多个上游 LLM 服务配置为互备关系，当某个服务限流或拒绝服务时，自动切换到下一个可用服务。切换过程对客户端完全透明，大幅提升访问稳定性。
- **统一别名，无感升级** — 上游模型升级后，通过设置别名以统一名称对外提供服务，客户端无需修改任何配置即可享受最新模型能力。
- **智能路由，按需分发** — 根据时间段、客户端请求 Token 估算值、请求关键字等条件，将请求智能转发至不同的上游 LLM。例如：将低 Token 请求路由到本地部署的 LLM 服务器以节省成本，将高 Token 请求路由到云端 LLM 以获得更强算力。
- **多模态能力扩展** — 当主力模型为纯文本模型（如 DeepSeek V4）时，代理可智能检测客户端请求是否包含图片内容，自动将请求转发至多模态模型（如 Qwen 3.6），让纯文本模型间接获得多模态处理能力。

## 架构

```
┌─────────────┐     ┌──────────────────────────────────────────┐     ┌──────────────┐
│   客户端     │────▶│              ModelProxy                  │────▶│  OpenAI API  │
│  (任意 SDK)  │     │                                          │     ├──────────────┤
└─────────────┘     │  ┌─────────┐  ┌──────────┐  ┌────────┐ │     │ Anthropic API│
                    │  │ 认证 &  │  │  智能    │  │ 重试   │ │     ├──────────────┤
                    │  │ 速率限制│  │  路由    │  │ 逻辑   │ │     │  Ollama API  │
                    │  └─────────┘  └──────────┘  └────────┘ │     ├──────────────┤
                    │                                          │     │  Minimax API │
                    │  ┌─────────────────────────────────────┐ │     ├──────────────┤
                    │  │       格式转换引擎                    │ │     │  自定义 API  │
                    │  │  OpenAI ↔ Anthropic ↔ Ollama        │ │     └──────────────┘
                    │  └─────────────────────────────────────┘ │
                    │                                          │
                    │  ┌──────────┐  ┌──────────┐  ┌────────┐ │
                    │  │  SQLite  │  │  内存    │  │ 审计   │ │
                    │  │  数据库  │  │  缓存    │  │ 日志   │ │
                    │  └──────────┘  └──────────┘  └────────┘ │
                    └──────────────────────────────────────────┘
```

## 快速开始

### 前置要求

- Rust 1.75+（从源码构建）
- SQLite3（由 Rust `sqlx` crate 内置）

### 构建

```bash
cargo build --release
```

二进制文件位于 `target/release/modelproxy`。

### 初始化

```bash
./modelproxy init
```

启动交互式配置向导，配置：
- 数据库路径
- 代理服务端口（默认：3000）
- 管理后台端口（默认：3001）
- 管理员密码

### 运行

```bash
./modelproxy
```

启动两个服务：
- **代理 API** — `http://127.0.0.1:3000`，OpenAI 兼容端点
- **管理后台** — `http://127.0.0.1:3001`，Web 管理界面

### GUI 模式

```bash
cargo run --bin modelproxy-gui --release
```

启动系统托盘应用，内嵌 Web 视图。

## 配置说明

配置存储在 `config.json`（首次运行自动生成）：

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
    "secret": "<自动生成>",
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

| 字段 | 说明 |
|------|------|
| `server.host/port` | 代理 API 监听地址 |
| `admin.host/port` | 管理后台监听地址 |
| `admin.allow_public_registration` | 是否允许公开注册 |
| `database.path` | SQLite 数据库文件路径 |
| `jwt.secret` | JWT 签名密钥（不安全时自动生成） |
| `proxy.request_timeout_secs` | 上游请求超时时间 |
| `proxy.max_request_body_bytes` | 请求体大小硬限制 |
| `proxy.max_text_request_body_bytes` | 纯文本请求体大小软限制 |
| `proxy.max_multimodal_request_body_bytes` | 多模态请求体大小软限制 |

## API 端点

### 代理 API（端口 3000）

OpenAI 兼容端点：

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/v1/models` | 获取可用模型列表 |
| POST | `/v1/chat/completions` | 聊天补全（OpenAI 格式） |
| POST | `/v1/completions` | 文本补全 |
| POST | `/v1/messages` | 聊天补全（Anthropic 格式透传） |

认证方式：`Authorization: Bearer <你的API密钥>`

### 管理 API（端口 3001）

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/auth/login` | 登录 |
| GET | `/api/upstreams` | 获取上游列表 |
| POST | `/api/upstreams` | 创建上游 |
| PUT | `/api/upstreams/:id` | 更新上游 |
| DELETE | `/api/upstreams/:id` | 删除上游 |
| POST | `/api/upstreams/:id/test` | 测试上游连通性 |
| GET | `/api/models` | 获取模型列表及可见性 |
| PUT | `/api/models/:upstream_id/:model_name` | 设置模型可见性 |
| GET | `/api/models/conditional-aliases` | 获取智能路由列表 |
| PUT | `/api/models/conditional-aliases/:alias` | 创建/更新智能路由 |
| DELETE | `/api/models/conditional-aliases/:alias` | 删除智能路由 |
| GET | `/api/users` | 获取用户列表 |
| POST | `/api/users` | 创建用户 |
| GET | `/api/keys` | 获取 API Key 列表 |
| POST | `/api/keys` | 创建 API Key |
| GET | `/api/usage/me` | 获取我的用量 |
| GET | `/api/conversations/me` | 获取我的会话记录 |
| GET | `/api/audit/proxy` | 获取代理审计日志 |
| GET | `/api/pricing` | 获取定价配置 |
| PUT | `/api/pricing/:model_key` | 设置模型定价 |

## 智能路由

智能路由允许根据条件将请求路由到不同的上游模型。当客户端请求一个模型别名时，代理按优先级顺序评估规则，将请求转发到第一个匹配的模型。

### 支持的条件类型

| 条件 | 说明 |
|------|------|
| `token_gt` | 输入 Token 数超过阈值 |
| `token_lt` | 输入 Token 数低于阈值 |
| `keyword` | 请求文本包含指定关键词 |
| `has_image` | 请求包含图片内容（多模态） |
| `time_range` | 当前时间在指定范围内 |

### 示例

创建智能路由别名 `smart-chat`：
- 如果输入 Token > 4000 → 路由到 `gpt-4-32k`
- 如果请求包含图片 → 路由到 `gpt-4-vision-preview`
- 如果关键词包含 "code" → 路由到 `codellama`
- 兜底 → 路由到 `gpt-3.5-turbo`

客户端只需在请求中使用 `model: "smart-chat"`，代理自动处理路由。

## 重试与降级

每个模型可配置重试设置：

| 设置 | 说明 |
|------|------|
| `retry_count` | 重试次数（0 = 不重试） |
| `retry_interval_seconds` | 重试基础间隔 |
| `retry_backoff_strategy` | `fixed`（固定）、`exponential`（指数退避）或 `exponential_jitter`（指数抖动） |
| `retry_max_interval_seconds` | 最大延迟上限 |
| `retry_failure_strategy` | `error`（返回错误）或 `route`（降级到其他模型） |
| `retry_fallback_upstream_id` | 降级目标上游 ID（strategy 为 route 时） |
| `retry_fallback_model_name` | 降级目标模型名称 |

## 使用示例

### Python (OpenAI SDK)

```python
from openai import OpenAI

client = OpenAI(
    api_key="your-modelproxy-api-key",
    base_url="http://127.0.0.1:3000/v1"
)

response = client.chat.completions.create(
    model="smart-chat",
    messages=[{"role": "user", "content": "你好！"}],
    stream=True
)

for chunk in response:
    print(chunk.choices[0].delta.content, end="")
```

### cURL

```bash
curl http://127.0.0.1:3000/v1/chat/completions \
  -H "Authorization: Bearer your-modelproxy-api-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "smart-chat",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": false
  }'
```

## 维护命令

```bash
# 将明文 API Key 迁移到加密存储
./modelproxy migrate-upstream-secrets

# 轮换加密密钥（重新加密所有密钥）
./modelproxy rotate-secrets
```

## 技术栈

- **后端**：Rust、Axum、SQLx、Reqwest、Tokio
- **前端**：原生 JavaScript、CSS（嵌入二进制）
- **数据库**：SQLite
- **加密**：AES-256-GCM（API Key 存储）
- **GUI**：Iced（可选）

## 许可证

本项目采用 [GNU 通用公共许可证 v3.0](LICENSE) 开源许可。
