-- ModelProxy SQLite 初始化脚本
-- 数据库版本: 1.0

-- 租户表
CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'active',
    plan_type TEXT NOT NULL DEFAULT 'free',
    billing_mode TEXT NOT NULL DEFAULT 'token',
    settings TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 用户表
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    status TEXT NOT NULL DEFAULT 'active',
    quota_limit INTEGER NOT NULL DEFAULT 0,
    quota_used INTEGER NOT NULL DEFAULT 0,
    daily_request_limit INTEGER NOT NULL DEFAULT 0,
    monthly_request_limit INTEGER NOT NULL DEFAULT 0,
    last_login TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    UNIQUE(tenant_id, username)
);

-- API密钥表
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    key_suffix TEXT,
    key_full TEXT,
    key_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    rpm_limit INTEGER NOT NULL DEFAULT 0,
    tpm_limit INTEGER NOT NULL DEFAULT 0,
    daily_limit INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT,
    last_used_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- 上游配置表
CREATE TABLE IF NOT EXISTS upstream_configs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    api_type TEXT NOT NULL DEFAULT 'openai',
    base_url TEXT NOT NULL,
    api_key_encrypted TEXT,
    models TEXT NOT NULL DEFAULT '',
    custom_headers TEXT NOT NULL DEFAULT '{}',
    priority INTEGER NOT NULL DEFAULT 1,
    weight INTEGER NOT NULL DEFAULT 100,
    rate_limit INTEGER,
    daily_request_limit INTEGER NOT NULL DEFAULT 2000,
    monthly_request_limit INTEGER NOT NULL DEFAULT 50000,
    daily_request_used INTEGER NOT NULL DEFAULT 0,
    monthly_request_used INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

-- 上游分组表
CREATE TABLE IF NOT EXISTS upstream_groups (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    balance_strategy TEXT NOT NULL DEFAULT 'priority',
    failover_enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

-- 上游分组成员表
CREATE TABLE IF NOT EXISTS upstream_group_members (
    id TEXT PRIMARY KEY,
    group_id TEXT NOT NULL,
    upstream_id TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 1,
    weight INTEGER NOT NULL DEFAULT 100,
    is_backup INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    FOREIGN KEY (group_id) REFERENCES upstream_groups(id),
    FOREIGN KEY (upstream_id) REFERENCES upstream_configs(id)
);

-- 模型可见性表
CREATE TABLE IF NOT EXISTS model_visibility (
    id TEXT PRIMARY KEY,
    upstream_id TEXT NOT NULL,
    model_name TEXT NOT NULL,
    model_alias TEXT,
    model_headers TEXT NOT NULL DEFAULT '{}',
    all_users_visible INTEGER NOT NULL DEFAULT 1,
    retry_count INTEGER NOT NULL DEFAULT 0,
    retry_interval_seconds INTEGER NOT NULL DEFAULT 0,
    retry_backoff_strategy TEXT NOT NULL DEFAULT 'fixed',
    retry_max_interval_seconds INTEGER NOT NULL DEFAULT 0,
    retry_failure_strategy TEXT NOT NULL DEFAULT 'error',
    retry_fallback_upstream_id TEXT,
    retry_fallback_model_name TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (upstream_id) REFERENCES upstream_configs(id),
    UNIQUE(upstream_id, model_name)
);

-- 模型可见性用户关联表
CREATE TABLE IF NOT EXISTS model_visibility_users (
    id TEXT PRIMARY KEY,
    visibility_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    FOREIGN KEY (visibility_id) REFERENCES model_visibility(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- 条件别名路由表
CREATE TABLE IF NOT EXISTS conditional_alias_routes (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    alias TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 1,
    upstream_id TEXT NOT NULL,
    model_name TEXT NOT NULL,
    min_input_tokens INTEGER,
    max_input_tokens INTEGER,
    keywords TEXT,
    has_image INTEGER NOT NULL DEFAULT 0,
    start_time TEXT,
    end_time TEXT,
    is_fallback INTEGER NOT NULL DEFAULT 0,
    all_users_visible INTEGER NOT NULL DEFAULT 1,
    user_ids TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (upstream_id) REFERENCES upstream_configs(id)
);

-- 会话表
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    api_key_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL UNIQUE,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    client_ip TEXT NOT NULL DEFAULT '',
    total_input_tokens INTEGER NOT NULL DEFAULT 0,
    total_output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (api_key_id) REFERENCES api_keys(id)
);

-- 会话消息表
CREATE TABLE IF NOT EXISTS conversation_messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    input_content TEXT NOT NULL DEFAULT '',
    output_content TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    model TEXT NOT NULL DEFAULT '',
    finish_reason TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);

-- 代理日志表
CREATE TABLE IF NOT EXISTS proxy_logs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    api_key_id TEXT NOT NULL,
    conversation_id TEXT,
    model TEXT NOT NULL,
    routed_model TEXT,
    provider TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'success',
    error_message TEXT,
    log_file TEXT,
    client_ip TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (api_key_id) REFERENCES api_keys(id)
);

-- 使用记录表
CREATE TABLE IF NOT EXISTS usage_records (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    api_key_id TEXT NOT NULL,
    conversation_id TEXT,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    billing_mode TEXT NOT NULL DEFAULT 'token',
    recorded_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (api_key_id) REFERENCES api_keys(id)
);

-- 系统设置表
CREATE TABLE IF NOT EXISTS system_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 创建索引
CREATE INDEX IF NOT EXISTS idx_users_tenant_id ON users(tenant_id);
CREATE INDEX IF NOT EXISTS idx_users_username ON users(tenant_id, username);
CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_upstream_configs_tenant_id ON upstream_configs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_model_visibility_upstream_id ON model_visibility(upstream_id);
CREATE INDEX IF NOT EXISTS idx_proxy_logs_tenant_id ON proxy_logs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_proxy_logs_created_at ON proxy_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_proxy_logs_user_id ON proxy_logs(user_id);
CREATE INDEX IF NOT EXISTS idx_conversations_user_id ON conversations(user_id);
CREATE INDEX IF NOT EXISTS idx_conversations_tenant_id ON conversations(tenant_id);
CREATE INDEX IF NOT EXISTS idx_usage_records_tenant_id ON usage_records(tenant_id);
CREATE INDEX IF NOT EXISTS idx_usage_records_user_id ON usage_records(user_id);
CREATE INDEX IF NOT EXISTS idx_conditional_alias_tenant ON conditional_alias_routes(tenant_id, alias);

-- 审计日志表
CREATE TABLE IF NOT EXISTS audit_logs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    details TEXT NOT NULL DEFAULT '{}',
    ip_address TEXT,
    user_agent TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_tenant_id ON audit_logs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_audit_logs_created_at ON audit_logs(created_at);

-- 插入默认租户
INSERT OR IGNORE INTO tenants (id, name, slug, status, plan_type, billing_mode, settings)
VALUES ('00000000-0000-0000-0000-000000000001', 'Default Tenant', 'default', 'active', 'pro', 'token', '{}');
