use sqlx::sqlite::SqlitePoolOptions;
use std::path::Path;
use std::time::Duration;

pub type DbPool = sqlx::SqlitePool;

pub async fn create_pool(db_path: &str) -> Result<DbPool, sqlx::Error> {
    if let Some(parent) = Path::new(db_path).parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(600))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("PRAGMA journal_mode = WAL")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA synchronous = NORMAL")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA busy_timeout = 10000")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA cache_size = -64000")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA journal_size_limit = 67108864")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA temp_store = MEMORY")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA mmap_size = 268435456")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA wal_autocheckpoint = 1000")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&format!("sqlite:{}?mode=rwc", db_path))
        .await?;

    Ok(pool)
}

pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    let migration_sql = include_str!("../../migrations/init.sql");
    
    for statement in migration_sql.split(';') {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement)
            .execute(pool)
            .await?;
    }

    run_schema_migrations(pool).await?;
    
    Ok(())
}

async fn run_schema_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    let columns_to_add = [
        ("upstream_configs", "api_type", "TEXT NOT NULL DEFAULT 'openai'"),
        ("upstream_configs", "custom_headers", "TEXT NOT NULL DEFAULT '{}'"),
        ("upstream_configs", "models", "TEXT NOT NULL DEFAULT ''"),
        ("upstream_configs", "priority", "INTEGER NOT NULL DEFAULT 1"),
        ("upstream_configs", "weight", "INTEGER NOT NULL DEFAULT 100"),
        ("upstream_configs", "rate_limit", "INTEGER"),
        ("upstream_configs", "daily_request_limit", "INTEGER"),
        ("upstream_configs", "monthly_request_limit", "INTEGER"),
        ("conversations", "created_at", "TEXT NOT NULL DEFAULT (datetime('now'))"),
        ("proxy_logs", "routed_model", "TEXT"),
        ("proxy_logs", "messages", "TEXT NOT NULL DEFAULT '[]'"),
        ("api_keys", "rpm_limit", "INTEGER NOT NULL DEFAULT 0"),
        ("api_keys", "tpm_limit", "INTEGER NOT NULL DEFAULT 0"),
        ("api_keys", "daily_limit", "INTEGER NOT NULL DEFAULT 0"),
        ("conditional_alias_routes", "start_time", "TEXT"),
        ("conditional_alias_routes", "end_time", "TEXT"),
        ("conditional_alias_routes", "has_image", "INTEGER NOT NULL DEFAULT 0"),
        ("model_visibility", "retry_count", "INTEGER NOT NULL DEFAULT 0"),
        ("model_visibility", "retry_interval_seconds", "INTEGER NOT NULL DEFAULT 0"),
        ("model_visibility", "retry_backoff_strategy", "TEXT NOT NULL DEFAULT 'fixed'"),
        ("model_visibility", "retry_max_interval_seconds", "INTEGER NOT NULL DEFAULT 0"),
        ("model_visibility", "retry_failure_strategy", "TEXT NOT NULL DEFAULT 'error'"),
        ("model_visibility", "retry_fallback_upstream_id", "TEXT"),
        ("model_visibility", "retry_fallback_model_name", "TEXT"),
        ("api_keys", "key_suffix", "TEXT"),
        ("api_keys", "key_full", "TEXT"),
    ];

    for (table, column, definition) in columns_to_add {
        let column_exists: bool = sqlx::query_scalar(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}')",
                table, column
            )
        )
        .fetch_one(pool)
        .await?;

        if !column_exists {
            sqlx::query(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table, column, definition
            ))
            .execute(pool)
            .await?;
        }
    }

    let audit_logs_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT name FROM sqlite_master WHERE type='table' AND name='audit_logs')",
    )
    .fetch_one(pool)
    .await?;

    if !audit_logs_exists {
        sqlx::query(
            r#"
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
            )
            "#,
        )
        .execute(pool)
        .await?;
    }

    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT name FROM sqlite_master WHERE type='table' AND name='system_settings')",
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS system_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(pool)
        .await?;
    }

    let conv_msg_unique: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pragma_index_list('conversation_messages') WHERE \"unique\" = 1 AND origin = 1)",
    )
    .fetch_one(pool)
    .await?;

    if conv_msg_unique {
        sqlx::query("ALTER TABLE conversation_messages RENAME TO _conversation_messages_old")
            .execute(pool)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE conversation_messages (
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
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO conversation_messages (id, conversation_id, input_content, output_content, input_tokens, output_tokens, model, finish_reason, created_at)
            SELECT id, conversation_id, input_content, output_content, input_tokens, output_tokens, model, finish_reason, created_at
            FROM _conversation_messages_old
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query("DROP TABLE _conversation_messages_old")
            .execute(pool)
            .await?;
    }

    Ok(())
}

pub mod api_keys;
pub mod audit;
pub mod conversations;
pub mod model_visibility;
pub mod tenants;
pub mod upstreams;
pub mod usage;
pub mod users;
