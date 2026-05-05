mod admin;
mod api_keys;
mod audit;
mod auth;
mod config;
mod db;
mod models;
mod proxy;
mod store;
mod usage;
mod utils;

use anyhow::{anyhow, Context};
use bcrypt::{hash, DEFAULT_COST};
use include_dir::{include_dir, Dir};
use rand::RngCore;
use std::io::{self, Write};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::Path,
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderValue, StatusCode, Uri,
    },
    middleware,
    response::Response,
    routing::{delete, get, post, put},
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::auth::JwtService;
use crate::config::Config;
use crate::proxy::{
    LoadBalancer, ProxyState, RateLimiter, UpstreamBlocker, UpstreamClient, UpstreamErrorLogger,
    UpstreamRateLimiter,
};
use crate::store::{AdminState, StoreManager};
use crate::utils::secrets::is_insecure_default_jwt_secret;

static EMBEDDED_STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static");
const APP_VERSION: &str = "1.0.01";

#[derive(Debug, Clone, Copy)]
enum MaintenanceCommand {
    MigrateUpstreamSecrets,
    RotateSecrets,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "modelproxy=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    tracing::info!("ModelProxy version: {}", APP_VERSION);

    let args = std::env::args().collect::<Vec<_>>();
    if let Some(init_args) = parse_init_args(args.clone())? {
        run_init(init_args).await?;
        return Ok(());
    }
    if let Some(command) = parse_maintenance_command(&args)? {
        let config = Config::load().map_err(|e| anyhow!(e.to_string()))?;
        run_maintenance_command(command, &config).await?;
        return Ok(());
    }

    let mut config = Config::load().map_err(|e| anyhow!(e.to_string()))?;
    if is_insecure_default_jwt_secret(&config.jwt.secret) {
        tracing::info!("JWT 密钥未配置或为默认弱密钥，正在自动生成高强度密钥并更新配置...");
        let new_secret = crate::utils::secrets::generate_secure_secret();
        config.jwt.secret = new_secret;
        
        // 如果没有设置上游加密密钥，也一并生成一个
        if config.jwt.upstream_key_secret.is_none() {
            config.jwt.upstream_key_secret = Some(crate::utils::secrets::generate_secure_secret());
        }

        // 获取保存路径
        let save_path = Config::get_save_path();

        config.save_to_file(save_path.to_str().unwrap()).map_err(|e| {
            anyhow!("自动生成 JWT 密钥后保存配置文件失败: {}", e)
        })?;
        tracing::info!("高强度密钥已生成并成功保存到 {}", save_path.display());
    }
    tracing::info!("Loaded configuration: {:?}", config);

    let db_path = config.database.get_db_path();
    let pool = db::create_pool(&db_path).await?;
    tracing::info!("Database pool created at {}", db_path);
    
    db::run_migrations(&pool).await?;
    tracing::info!("Database migrations completed");

    if !db::users::admin_exists(&pool).await? {
        tracing::info!("首次启动：未检测到管理员账户，请设置管理员密码");
        let admin_password = prompt_admin_password()?;
        let password_hash = hash(&admin_password, DEFAULT_COST)
            .map_err(|e| anyhow!("密码加密失败: {}", e))?;
        db::users::create_admin(&pool, "admin", &password_hash, "admin@localhost").await?;
        tracing::info!("管理员账户已创建 (用户名: admin)");
    }

    let migrated_upstream_secrets = db::upstreams::migrate_plaintext_api_keys(&pool).await?;
    if migrated_upstream_secrets > 0 {
        tracing::info!(
            "Migrated {} legacy upstream API keys to encrypted storage",
            migrated_upstream_secrets
        );
    }

    let proxy_log_path = config.audit_log.get_resolved_path();
    let store_manager = Arc::new(
        StoreManager::new(pool.clone())
            .with_audit_log_writer(&config.audit_log.path)?
            .with_proxy_log_writer(&proxy_log_path)?
    );
    tracing::info!("Store manager initialized with audit log path: {}", config.audit_log.path);

    store_manager.init_from_sqlite(&pool).await?;
    tracing::info!("Memory cache loaded from SQLite");

    let jwt_service = Arc::new(JwtService::new(&config.jwt));

    let client = UpstreamClient::new(&config.proxy)?;
    let client_arc = Arc::new(client.clone());
    let rate_limiter = Arc::new(RateLimiter::new(
        config.rate_limit.window_size_secs,
        config.rate_limit.cleanup_interval_secs,
    ));
    let upstream_rate_limiter = Arc::new(UpstreamRateLimiter::new());
    let load_balancer = LoadBalancer::new();
    let blocker = UpstreamBlocker::new(Some(600)); // 10分钟屏蔽时间

    let error_logger = UpstreamErrorLogger::new(&std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        .map_err(|e| anyhow!("Failed to create error logger: {}", e))?;
    tracing::info!("Error log will be written to error.log in current directory");

    let proxy_state = ProxyState {
        store: store_manager.clone(),
        client,
        rate_limiter,
        upstream_rate_limiter: upstream_rate_limiter.clone(),
        load_balancer,
        config: config.proxy.clone(),
        blocker,
        error_logger: Some(error_logger),
    };

    // 代理服务路由 (端口 3000)
    let proxy_routes = Router::new()
        .route("/models", get(proxy::handlers::list_models))
        .route("/chat/completions", post(proxy_handler))
        .route("/completions", post(proxy_handler))
        .route("/messages", post(proxy_handler))
        .with_state(proxy_state);

    let proxy_app = Router::new()
        .nest("/v1", proxy_routes)
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    // 管理后台路由 (端口 3001)
    let public_auth_routes = {
        let routes = Router::new().route("/login", post(auth::handlers::login));
        let routes = if config.admin.allow_public_registration {
            routes.route("/register", post(auth::handlers::register))
        } else {
            routes
        };
        routes.with_state((pool.clone(), jwt_service.clone(), store_manager.clone()))
    };

    let protected_auth_routes = Router::new()
        .route("/logout", post(auth::handlers::logout))
        .route("/me", get(auth::handlers::get_current_user))
        .route("/change-password", post(auth::handlers::change_password))
        .with_state((pool.clone(), jwt_service.clone(), store_manager.clone()));

    let admin_state = AdminState::new(
        pool.clone(),
        store_manager.clone(),
        upstream_rate_limiter.clone(),
    );

    let api_key_routes = Router::new()
        .route("/", get(api_keys::list_my_keys).post(api_keys::create_key))
        .route(
            "/:id",
            get(api_keys::get_key)
                .put(api_keys::update_key)
                .delete(api_keys::delete_key),
        )
        .route("/all", get(api_keys::list_all_keys))
        .with_state(admin_state.clone());

    let usage_routes = Router::new()
        .route("/me", get(usage::get_my_usage))
        .route("/me/quota", get(usage::get_my_quota))
        .route("/all", get(usage::get_all_usage))
        .route("/user", get(usage::get_user_usage))
        .with_state(admin_state.clone());

    let audit_routes = Router::new()
        .route("/proxy", get(audit::list_proxy_audit_logs))
        .route("/proxy/export", get(audit::export_proxy_audit_logs))
        .route("/proxy/:id", get(audit::get_proxy_audit_log))
        .route("/", get(audit::list_audit_logs))
        .route("/:id", get(audit::get_audit_log))
        .with_state(admin_state.clone());

    let user_admin_routes = Router::new()
        .route("/", get(admin::list_users).post(admin::create_user))
        .route(
            "/:id",
            get(admin::get_user)
                .put(admin::update_user)
                .delete(admin::delete_user),
        )
        .route("/:id/reset-password", post(admin::reset_user_password))
        .with_state(admin_state.clone());

    let upstream_admin_routes = Router::new()
        .route("/", get(admin::list_upstreams).post(admin::create_upstream))
        .route(
            "/:id",
            get(admin::get_upstream)
                .put(admin::update_upstream)
                .delete(admin::delete_upstream),
        )
        .route("/:id/test", post(admin::test_upstream))
        .route("/:id/test-model", post(admin::test_upstream_model))
        .route("/:id/status", put(admin::update_upstream_status))
        .route("/:id/api-key", get(admin::get_upstream_api_key))
        .route(
            "/groups",
            get(admin::list_upstream_groups).post(admin::create_upstream_group),
        )
        .route("/groups/:id", delete(admin::delete_upstream_group))
        .with_state(admin_state.clone());

    let model_admin_routes = Router::new()
        .route("/", get(admin::list_models))
        .route("/fetch", get(admin::fetch_upstream_models))
        .route("/my", get(admin::get_user_models))
        .route("/conditional-aliases", get(admin::list_conditional_aliases))
        .route(
            "/conditional-aliases/:alias",
            put(admin::set_conditional_alias).delete(admin::delete_conditional_alias),
        )
        .route(
            "/conditional-aliases/:alias/visibility",
            put(admin::set_conditional_alias_visibility),
        )
        .route("/refresh", post(admin::refresh_cache))
        .route("/:upstream_id/:model_name", put(admin::set_visibility))
        .with_state((pool.clone(), client_arc.clone(), store_manager.clone()));

    let settings_routes = Router::new()
        .route("/", get(admin::get_settings).put(admin::update_settings))
        .with_state(admin_state.clone());

    let public_settings_routes = Router::new()
        .route("/public", get(admin::get_public_settings))
        .with_state(admin_state.clone());

    let protected_routes = Router::new()
        .nest("/keys", api_key_routes)
        .nest("/usage", usage_routes)
        .nest("/audit", audit_routes)
        .nest("/users", user_admin_routes)
        .nest("/upstreams", upstream_admin_routes)
        .nest("/models", model_admin_routes)
        .nest("/settings", settings_routes)
        .layer(middleware::from_fn_with_state(
            (jwt_service.clone(), store_manager.clone()),
            auth::middleware::auth_middleware,
        ));

    let admin_app = Router::new()
        .nest("/auth", public_auth_routes)
        .nest("/api/settings", public_settings_routes)
        .nest("/api", protected_routes.clone())
        .nest(
            "/api/auth",
            protected_auth_routes.layer(middleware::from_fn_with_state(
                (jwt_service.clone(), store_manager.clone()),
                auth::middleware::auth_middleware,
            )),
        )
        .route("/static/*path", get(serve_embedded_static))
        .route("/", get(serve_embedded_index))
        .fallback(get(serve_embedded_fallback))
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    // 启动代理服务
    let proxy_addr = config.server.addr();
    let proxy_listener = tokio::net::TcpListener::bind(proxy_addr).await?;
    tracing::info!("Proxy server listening on {}", proxy_addr);

    // 启动管理后台服务
    let admin_addr = config.admin.addr();
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    tracing::info!("Admin server listening on {}", admin_addr);

    let store_for_shutdown = store_manager.clone();
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let shutdown_rx1 = shutdown_tx.subscribe();
    let shutdown_rx2 = shutdown_tx.subscribe();

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Shutdown signal received, flushing data...");
        store_for_shutdown.flush_last_used_at().await;
        tracing::info!("Data flushed, shutting down...");
        let _ = shutdown_tx.send(());
    });

    let mut rx1 = shutdown_rx1;
    let mut rx2 = shutdown_rx2;

    tokio::try_join!(
        axum::serve(
            proxy_listener,
            proxy_app.into_make_service_with_connect_info::<std::net::SocketAddr>()
        )
        .with_graceful_shutdown(async move { let _ = rx1.recv().await; }),
        axum::serve(admin_listener, admin_app)
            .with_graceful_shutdown(async move { let _ = rx2.recv().await; }),
    )?;

    Ok(())
}

async fn serve_embedded_static(Path(path): Path<String>) -> Response {
    let normalized = path.trim_start_matches('/');
    if normalized.is_empty() || normalized.contains("..") {
        return embedded_not_found();
    }
    embedded_file_response(normalized)
}

async fn serve_embedded_index() -> Response {
    embedded_file_response("index.html")
}

async fn serve_embedded_fallback(uri: Uri) -> Response {
    let normalized = uri.path().trim_start_matches('/');
    if !normalized.is_empty() && !normalized.contains("..") {
        let maybe = if normalized.starts_with("static/") {
            normalized.trim_start_matches("static/")
        } else {
            normalized
        };
        if EMBEDDED_STATIC_DIR.get_file(maybe).is_some() {
            return embedded_file_response(maybe);
        }
    }
    embedded_file_response("index.html")
}

fn embedded_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not Found"))
        .expect("build response")
}

fn embedded_file_response(path: &str) -> Response {
    let Some(file) = EMBEDDED_STATIC_DIR.get_file(path) else {
        return embedded_not_found();
    };

    let mut response = Response::new(Body::from(file.contents().to_vec()));
    if let Ok(v) = HeaderValue::from_str(embedded_content_type(path)) {
        response.headers_mut().insert(CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(embedded_cache_control(path)) {
        response.headers_mut().insert(CACHE_CONTROL, v);
    }
    response
}

fn embedded_content_type(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}

fn embedded_cache_control(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".css") || path.ends_with(".js") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}

#[derive(Debug, Clone)]
struct InitArgs {
    db_path: String,
    proxy_port: u16,
    admin_port: u16,
    admin_password: String,
}

fn parse_init_args(args: Vec<String>) -> anyhow::Result<Option<InitArgs>> {
    if args.len() < 2 || args[1] != "init" {
        return Ok(None);
    }
    if args.len() > 2 {
        return Err(anyhow!(
            "init 已改为向导模式，请直接执行: cargo run --bin modelproxy -- init"
        ));
    }
    Ok(Some(prompt_init_args()?))
}

fn parse_maintenance_command(args: &[String]) -> anyhow::Result<Option<MaintenanceCommand>> {
    if args.len() < 2 {
        return Ok(None);
    }

    match args[1].as_str() {
        "migrate-upstream-secrets" => {
            if args.len() > 2 {
                return Err(anyhow!(
                    "migrate-upstream-secrets 不接受额外参数，请直接执行: cargo run --bin modelproxy -- migrate-upstream-secrets"
                ));
            }
            Ok(Some(MaintenanceCommand::MigrateUpstreamSecrets))
        }
        "rotate-secrets" => {
            if args.len() > 2 {
                return Err(anyhow!(
                    "rotate-secrets 不接受额外参数，请直接执行: cargo run --bin modelproxy -- rotate-secrets"
                ));
            }
            Ok(Some(MaintenanceCommand::RotateSecrets))
        }
        _ => Ok(None),
    }
}

fn prompt_init_args() -> anyhow::Result<InitArgs> {
    println!();
    println!("ModelProxy 初始化向导");
    println!("请按提示输入配置，直接回车可使用默认值");
    println!();

    let db_path = prompt_with_default("数据库文件路径", "data/modelproxy.db")?;
    let proxy_port = prompt_u16_with_default("代理服务端口", 3000)?;
    let admin_port = prompt_u16_with_default("管理后台端口", 3001)?;
    let admin_password = prompt_required("管理员密码")?;

    Ok(InitArgs {
        db_path,
        proxy_port,
        admin_port,
        admin_password,
    })
}

fn prompt_with_default(label: &str, default_value: &str) -> anyhow::Result<String> {
    print!("{} [{}]: ", label, default_value);
    io::stdout().flush().context("输出提示失败")?;
    let mut input = String::new();
    io::stdin().read_line(&mut input).context("读取输入失败")?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default_value.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn prompt_required(label: &str) -> anyhow::Result<String> {
    loop {
        print!("{}: ", label);
        io::stdout().flush().context("输出提示失败")?;
        let mut input = String::new();
        io::stdin().read_line(&mut input).context("读取输入失败")?;
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        println!("该项不能为空，请重新输入");
    }
}

fn prompt_u16_with_default(label: &str, default_value: u16) -> anyhow::Result<u16> {
    loop {
        let value = prompt_with_default(label, &default_value.to_string())?;
        match value.parse::<u16>() {
            Ok(v) => return Ok(v),
            Err(_) => println!("请输入有效的端口号"),
        }
    }
}

fn prompt_admin_password() -> anyhow::Result<String> {
    loop {
        print!("请输入管理员密码 (至少6位): ");
        io::stdout().flush().context("输出提示失败")?;
        let password = rpassword::read_password().context("读取密码失败")?;
        if password.len() < 6 {
            println!("密码长度不能少于6位，请重新输入");
            continue;
        }
        print!("请确认管理员密码: ");
        io::stdout().flush().context("输出提示失败")?;
        let confirm = rpassword::read_password().context("读取密码失败")?;
        if password != confirm {
            println!("两次输入的密码不一致，请重新输入");
            continue;
        }
        return Ok(password);
    }
}

fn generate_secure_token(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn run_maintenance_command(
    command: MaintenanceCommand,
    config: &Config,
) -> anyhow::Result<()> {
    match command {
        MaintenanceCommand::MigrateUpstreamSecrets => {
            if is_insecure_default_jwt_secret(&config.jwt.secret) {
                tracing::warn!(
                    "当前仍在使用默认或弱 JWT 密钥，旧上游密钥将基于当前密钥迁移；建议迁移完成后尽快更换并固定 MODELPROXY_UPSTREAM_KEY_SECRET"
                );
            }
            let db_path = config.database.get_db_path();
            let pool = db::create_pool(&db_path).await?;
            let migrated = db::upstreams::migrate_plaintext_api_keys(&pool).await?;
            tracing::info!(
                "Upstream API key migration completed, migrated {} record(s)",
                migrated
            );
            pool.close().await;
        }
        MaintenanceCommand::RotateSecrets => {
            let db_path = config.database.get_db_path();
            let pool = db::create_pool(&db_path).await?;
            let old_jwt_secret = config.jwt.secret.clone();
            let old_upstream_secret = config
                .jwt
                .upstream_key_secret
                .clone()
                .unwrap_or_else(|| old_jwt_secret.clone());

            let new_jwt_secret = crate::utils::secrets::generate_secure_secret();
            let new_upstream_secret = crate::utils::secrets::generate_secure_secret();

            tracing::info!("Starting smooth secret rotation...");

            // Re-encrypt existing keys using new upstream secret
            let reencrypted_count =
                db::upstreams::reencrypt_api_keys(&pool, &old_upstream_secret, &new_upstream_secret)
                    .await?;

            tracing::info!(
                "Re-encrypted {} upstream keys with new secret",
                reencrypted_count
            );

            // Update config file
            let mut new_config = config.clone();
            new_config.jwt.secret = new_jwt_secret;
            new_config.jwt.upstream_key_secret = Some(new_upstream_secret);

            let save_path = Config::get_save_path();
            new_config
                .save_to_file(save_path.to_str().unwrap())
                .map_err(|e| anyhow!(e.to_string()))?;

            tracing::info!("Secret rotation completed. Config updated successfully.");
            pool.close().await;
        }
    }
    Ok(())
}

async fn run_init(args: InitArgs) -> anyhow::Result<()> {
    if let Some(parent) = std::path::Path::new(&args.db_path).parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .context("创建数据库目录失败")?;
        }
    }

    let pool = db::create_pool(&args.db_path).await
        .context("创建数据库连接失败")?;

    db::run_migrations(&pool).await
        .context("执行数据库迁移失败")?;

    let admin_hash = hash(&args.admin_password, DEFAULT_COST).context("管理员密码加密失败")?;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = datetime('now') WHERE username = 'admin'")
        .bind(admin_hash)
        .execute(&pool)
        .await
        .context("更新管理员密码失败")?;

    let config = Config {
        server: crate::config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: args.proxy_port,
            workers: 8,
        },
        admin: crate::config::AdminServerConfig {
            host: "127.0.0.1".to_string(),
            port: args.admin_port,
            base_url: None,
            allow_public_registration: false,
        },
        database: crate::config::DatabaseConfig {
            path: args.db_path.clone(),
            max_connections: 10,
        },
        jwt: crate::config::JwtConfig {
            secret: generate_secure_token(32),
            expiration_hours: 24,
            upstream_key_secret: Some(generate_secure_token(32)),
        },
        proxy: crate::config::ProxyConfig {
            request_timeout_secs: 300,
            connect_timeout_secs: 30,
            max_idle_connections: 200,
            max_request_body_bytes: 32 * 1024 * 1024,
            max_text_request_body_bytes: 2 * 1024 * 1024,
            max_multimodal_request_body_bytes: 20 * 1024 * 1024,
        },
        rate_limit: crate::config::RateLimitConfig {
            cleanup_interval_secs: 60,
            window_size_secs: 60,
        },
        audit_log: crate::config::AuditLogConfig::default(),
    };
    let save_path = Config::get_save_path();
    config
        .save_to_file(save_path.to_str().unwrap())
        .map_err(|e| anyhow!(e.to_string()))?;

    println!();
    println!("初始化完成！");
    println!("数据库文件: {}", args.db_path);
    println!("配置文件: {}", save_path.display());
    println!("代理服务端口: {}", args.proxy_port);
    println!("管理后台端口: {}", args.admin_port);
    println!();
    println!("请运行 'modelproxy' 启动服务");

    Ok(())
}

use crate::proxy::handlers::proxy_handler;
