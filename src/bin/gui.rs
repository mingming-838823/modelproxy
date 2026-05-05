#![windows_subsystem = "windows"]

use std::sync::{Arc, Mutex, OnceLock};
use std::io::Write;

use include_dir::{include_dir, Dir};
use axum::body::Body;
use axum::extract::Path as AxumPath;
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use iced::{
    Element, Length, Settings, Subscription, Task,
    alignment, font, time,
    widget::{
        button, checkbox, column, container, row, scrollable, text, text_input,
        Space,
    },
    Color,
};

static EMBEDDED_STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static");

static GUI_LOG_WRITER: OnceLock<Mutex<GuiLogFileWriter>> = OnceLock::new();
static GUI_LOG_SENDER: OnceLock<std::sync::mpsc::Sender<LogEntry>> = OnceLock::new();

struct GuiLogFileWriter {
    file: std::fs::File,
    date: String,
    base_path: String,
}

impl GuiLogFileWriter {
    fn new(base_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let dir = std::path::Path::new(base_path);
        std::fs::create_dir_all(dir)?;
        let log_path = dir.join(format!("{}.log", today));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        Ok(Self {
            file,
            date: today,
            base_path: base_path.to_string(),
        })
    }

    fn write_log(&mut self, msg: &str) {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if today != self.date {
            if let Ok(new_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::path::Path::new(&self.base_path).join(format!("{}.log", today)))
            {
                self.file = new_file;
                self.date = today;
            }
        }
        let _ = writeln!(self.file, "{}", msg);
        let _ = self.file.flush();
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum LogLevel {
    Error,
    Warn,
    Info,
}

impl LogLevel {
    fn text(self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN",
            LogLevel::Info => "INFO",
        }
    }
}

#[derive(Clone)]
struct LogEntry {
    level: LogLevel,
    message: String,
}

struct GuiTracingLayer {
    sender: std::sync::mpsc::Sender<LogEntry>,
}

impl GuiTracingLayer {
    fn new(sender: std::sync::mpsc::Sender<LogEntry>) -> Self {
        Self { sender }
    }
}

impl<S> tracing_subscriber::Layer<S> for GuiTracingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let target = event.metadata().module_path().unwrap_or("");
        if !target.starts_with("modelproxy") && !target.starts_with("modelproxy_gui") {
            return;
        }

        let mut visitor = StringVisitor::default();
        event.record(&mut visitor);
        let msg = visitor.0;
        if msg.is_empty() {
            return;
        }

        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG | tracing::Level::TRACE => return,
        };

        if let Some(writer) = GUI_LOG_WRITER.get() {
            if let Ok(mut w) = writer.lock() {
                w.write_log(&format!("[{}] {}", level.text(), msg));
            }
        }

        let _ = self.sender.send(LogEntry { level, message: msg });
    }
}

#[derive(Default)]
struct StringVisitor(String);

impl tracing::field::Visit for StringVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
}

#[derive(Clone, PartialEq)]
enum ServiceStatus {
    Stopped,
    Running,
    Error,
}

impl ServiceStatus {
    fn text(&self) -> &str {
        match self {
            ServiceStatus::Stopped => "Stopped",
            ServiceStatus::Running => "Running",
            ServiceStatus::Error => "Error",
        }
    }

    fn color(&self) -> Color {
        match self {
            ServiceStatus::Stopped => Color::from_rgb(0.58, 0.64, 0.72),
            ServiceStatus::Running => Color::from_rgb(0.13, 0.77, 0.37),
            ServiceStatus::Error => Color::from_rgb(0.94, 0.27, 0.27),
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    StartService,
    StopService,
    ToggleSettings,
    SaveSettings,
    CancelSettings,
    SettingsProxyPortChanged(String),
    SettingsAdminPortChanged(String),
    SettingsLocalhostOnlyChanged(bool),
    SettingsAdminLocalhostOnlyChanged(bool),
    ToggleLogFilterError,
    ToggleLogFilterWarn,
    ToggleLogFilterInfo,
    OpenAdminPanel,
    ClearLogs,
    CopyLogs,
    SetupPasswordChanged(String),
    SetupConfirmPasswordChanged(String),
    ConfirmSetupPassword,
}

struct AppState {
    status: ServiceStatus,
    logs: Vec<LogEntry>,
    log_filter_error: bool,
    log_filter_warn: bool,
    log_filter_info: bool,
    proxy_port: u16,
    admin_port: u16,
    proxy_localhost_only: bool,
    admin_localhost_only: bool,
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    server_thread: Option<std::thread::JoinHandle<()>>,
    show_settings: bool,
    show_setup_password: bool,
    setup_password: String,
    setup_confirm_password: String,
    setup_error: String,
    settings_proxy_port: String,
    settings_admin_port: String,
    settings_localhost_only: bool,
    settings_admin_localhost_only: bool,
    settings_error: String,
    log_receiver: Option<std::sync::mpsc::Receiver<LogEntry>>,
    password_request: Option<Arc<Mutex<Option<std::sync::mpsc::SyncSender<String>>>>>,
}

impl AppState {
    fn new(log_receiver: std::sync::mpsc::Receiver<LogEntry>) -> Self {
        let config = load_configuration().unwrap_or(AppConfig {
            proxy_port: 3000,
            admin_port: 3001,
            proxy_localhost_only: false,
            admin_localhost_only: true,
        });
        Self {
            status: ServiceStatus::Stopped,
            logs: Vec::new(),
            log_filter_error: true,
            log_filter_warn: true,
            log_filter_info: true,
            proxy_port: config.proxy_port,
            admin_port: config.admin_port,
            proxy_localhost_only: config.proxy_localhost_only,
            admin_localhost_only: config.admin_localhost_only,
            shutdown_tx: None,
            server_thread: None,
            show_settings: false,
            show_setup_password: false,
            setup_password: String::new(),
            setup_confirm_password: String::new(),
            setup_error: String::new(),
            settings_proxy_port: config.proxy_port.to_string(),
            settings_admin_port: config.admin_port.to_string(),
            settings_localhost_only: config.proxy_localhost_only,
            settings_admin_localhost_only: config.admin_localhost_only,
            settings_error: String::new(),
            log_receiver: Some(log_receiver),
            password_request: None,
        }
    }

    fn add_log(&mut self, level: LogLevel, msg: String) {
        let ts = chrono::Local::now().format("%H:%M:%S");
        self.logs.push(LogEntry { level, message: format!("[{}] {}", ts, msg) });
        if self.logs.len() > 2000 {
            self.logs.drain(0..self.logs.len() - 1000);
        }
    }

    fn start_service(&mut self) {
        if self.status == ServiceStatus::Running {
            return;
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);
        self.status = ServiceStatus::Running;

        let log_tx = GUI_LOG_SENDER.get().cloned()
            .unwrap_or_else(|| {
                let (tx, _) = std::sync::mpsc::channel();
                tx
            });

        self.add_log(LogLevel::Info, "Service starting...".to_string());

        let proxy_port = self.proxy_port;
        let admin_port = self.admin_port;
        let proxy_localhost_only = self.proxy_localhost_only;
        let admin_localhost_only = self.admin_localhost_only;

        let password_request = Arc::new(Mutex::new(None::<std::sync::mpsc::SyncSender<String>>));
        let password_request_for_thread = password_request.clone();

        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(run_service(
                shutdown_rx,
                log_tx,
                proxy_port,
                admin_port,
                proxy_localhost_only,
                admin_localhost_only,
                password_request_for_thread,
            ));
            if let Err(e) = result {
                eprintln!("Service error: {}", e);
            }
        });

        self.server_thread = Some(thread);
        self.password_request = Some(password_request);
    }

    fn stop_service(&mut self) {
        if self.status != ServiceStatus::Running {
            return;
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        self.add_log(LogLevel::Info, "Service stopping...".to_string());
    }

    fn check_service_finished(&mut self) {
        if self.status == ServiceStatus::Running {
            if let Some(thread) = self.server_thread.take() {
                if thread.is_finished() {
                    self.status = ServiceStatus::Stopped;
                    self.add_log(LogLevel::Info, "Service stopped".to_string());
                } else {
                    self.server_thread = Some(thread);
                }
            }
        }
    }

    fn drain_logs(&mut self) -> Vec<LogEntry> {
        let mut new_logs = Vec::new();
        if let Some(rx) = &self.log_receiver {
            for _ in 0..100 {
                match rx.try_recv() {
                    Ok(entry) => new_logs.push(entry),
                    Err(_) => break,
                }
            }
        }
        new_logs
    }

    fn poll(&mut self) {
        let new_logs = self.drain_logs();
        for entry in new_logs {
            self.add_log(entry.level, entry.message);
        }
        self.check_service_finished();

        if !self.show_setup_password {
            if let Some(ref password_request) = self.password_request {
                if password_request.lock().ok().map_or(false, |req| req.is_some()) {
                    self.show_setup_password = true;
                    self.setup_password.clear();
                    self.setup_confirm_password.clear();
                    self.setup_error.clear();
                }
            }
        }
    }
}

struct ModelProxy {
    state: AppState,
}

impl ModelProxy {
    fn new(log_receiver: std::sync::mpsc::Receiver<LogEntry>) -> Self {
        Self {
            state: AppState::new(log_receiver),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.state.poll();
            }
            Message::StartService => {
                self.state.start_service();
            }
            Message::StopService => {
                self.state.stop_service();
            }
            Message::ToggleSettings => {
                if self.state.show_settings {
                    self.state.show_settings = false;
                } else {
                    self.state.settings_proxy_port = self.state.proxy_port.to_string();
                    self.state.settings_admin_port = self.state.admin_port.to_string();
                    self.state.settings_localhost_only = self.state.proxy_localhost_only;
                    self.state.settings_admin_localhost_only = self.state.admin_localhost_only;
                    self.state.settings_error.clear();
                    self.state.show_settings = true;
                }
            }
            Message::SaveSettings => {
                let proxy_port: u16 = match self.state.settings_proxy_port.parse() {
                    Ok(p) if p > 0 => p,
                    _ => {
                        self.state.settings_error = "Invalid proxy port (1-65535)".to_string();
                        return Task::none();
                    }
                };
                let admin_port: u16 = match self.state.settings_admin_port.parse() {
                    Ok(p) if p > 0 => p,
                    _ => {
                        self.state.settings_error = "Invalid admin port (1-65535)".to_string();
                        return Task::none();
                    }
                };
                if proxy_port == admin_port {
                    self.state.settings_error = "Proxy and admin ports must be different".to_string();
                    return Task::none();
                }

                let proxy_host = if self.state.settings_localhost_only { "127.0.0.1" } else { "0.0.0.0" };
                let admin_host = if self.state.settings_admin_localhost_only { "127.0.0.1" } else { "0.0.0.0" };
                match save_configuration(proxy_port, admin_port, proxy_host, admin_host) {
                    Ok(_) => {
                        self.state.proxy_port = proxy_port;
                        self.state.admin_port = admin_port;
                        self.state.proxy_localhost_only = self.state.settings_localhost_only;
                        self.state.admin_localhost_only = self.state.settings_admin_localhost_only;
                        self.state.add_log(LogLevel::Info, format!(
                            "Configuration saved: Proxy={}:{} Admin={}:{}",
                            proxy_host, proxy_port, admin_host, admin_port
                        ));
                        self.state.add_log(LogLevel::Warn, "Please restart service for changes to take effect.".to_string());
                        self.state.show_settings = false;
                    }
                    Err(e) => {
                        self.state.settings_error = format!("Failed to save: {}", e);
                    }
                }
            }
            Message::CancelSettings => {
                self.state.show_settings = false;
            }
            Message::SettingsProxyPortChanged(v) => {
                self.state.settings_proxy_port = v;
            }
            Message::SettingsAdminPortChanged(v) => {
                self.state.settings_admin_port = v;
            }
            Message::SettingsLocalhostOnlyChanged(v) => {
                self.state.settings_localhost_only = v;
            }
            Message::SettingsAdminLocalhostOnlyChanged(v) => {
                self.state.settings_admin_localhost_only = v;
            }
            Message::ToggleLogFilterError => {
                self.state.log_filter_error = !self.state.log_filter_error;
            }
            Message::ToggleLogFilterWarn => {
                self.state.log_filter_warn = !self.state.log_filter_warn;
            }
            Message::ToggleLogFilterInfo => {
                self.state.log_filter_info = !self.state.log_filter_info;
            }
            Message::OpenAdminPanel => {
                let url = format!("http://127.0.0.1:{}", self.state.admin_port);
                self.state.add_log(LogLevel::Info, format!("Opening admin panel: {}", url));
                if let Err(e) = open::that(&url) {
                    self.state.add_log(LogLevel::Error, format!("Failed to open admin panel: {}", e));
                }
            }
            Message::ClearLogs => {
                self.state.logs.clear();
            }
            Message::CopyLogs => {
                let filtered: Vec<String> = self.state.logs.iter()
                    .filter(|e| match e.level {
                        LogLevel::Error => self.state.log_filter_error,
                        LogLevel::Warn => self.state.log_filter_warn,
                        LogLevel::Info => self.state.log_filter_info,
                    })
                    .map(|e| format!("[{}] {}", e.level.text(), e.message))
                    .collect();
                return iced::clipboard::write(filtered.join("\n"));
            }
            Message::SetupPasswordChanged(v) => {
                self.state.setup_password = v;
            }
            Message::SetupConfirmPasswordChanged(v) => {
                self.state.setup_confirm_password = v;
            }
            Message::ConfirmSetupPassword => {
                if self.state.setup_password.len() < 6 {
                    self.state.setup_error = "Password must be at least 6 characters".to_string();
                    return Task::none();
                }
                if self.state.setup_password != self.state.setup_confirm_password {
                    self.state.setup_error = "Passwords do not match".to_string();
                    return Task::none();
                }

                let password = self.state.setup_password.clone();
                if let Some(ref password_request) = self.state.password_request {
                    if let Ok(mut req) = password_request.lock() {
                        if let Some(tx) = req.take() {
                            let _ = tx.send(password);
                        }
                    }
                }
                self.state.show_setup_password = false;
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.state.show_setup_password {
            self.view_setup_password()
        } else if self.state.show_settings {
            self.view_settings()
        } else {
            self.view_main()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(std::time::Duration::from_millis(200)).map(|_| Message::Tick)
    }

    fn title(&self) -> String {
        "ModelProxy".to_string()
    }
}

impl ModelProxy {
    fn view_main(&self) -> Element<'_, Message> {
        let is_running = self.state.status == ServiceStatus::Running;

        let toolbar = container(
            row![
                text("ModelProxy").size(20).font(font::Font::DEFAULT),
                Space::with_width(Length::Fill),
                text(format!("Status: {}", self.state.status.text()))
                    .color(self.state.status.color()),
                Space::with_width(10),
                button(if is_running { "Stop" } else { "Start" })
                    .on_press(if is_running { Message::StopService } else { Message::StartService }),
                Space::with_width(5),
                button("Settings").on_press(Message::ToggleSettings),
                Space::with_width(5),
                button("Admin Panel").on_press(Message::OpenAdminPanel),
            ]
            .align_y(alignment::Vertical::Center)
            .spacing(5),
        )
        .padding(8)
        .style(container::rounded_box);

        let filtered_lines: String = self.state.logs.iter()
            .filter(|e| match e.level {
                LogLevel::Error => self.state.log_filter_error,
                LogLevel::Warn => self.state.log_filter_warn,
                LogLevel::Info => self.state.log_filter_info,
            })
            .map(|e| e.message.as_str())
            .collect::<Vec<&str>>()
            .join("\n");

        let log_display = scrollable(
            text(filtered_lines)
                .font(font::Font::MONOSPACE)
                .size(12)
                .color(Color::from_rgb(0.8, 0.8, 0.8))
                .width(Length::Fill),
        )
        .anchor_bottom();

        let log_filter_bar = container(
            row![
                text("Log:").size(12),
                Space::with_width(5),
                checkbox("Error", self.state.log_filter_error)
                    .on_toggle(|_| Message::ToggleLogFilterError),
                Space::with_width(5),
                checkbox("Warn", self.state.log_filter_warn)
                    .on_toggle(|_| Message::ToggleLogFilterWarn),
                Space::with_width(5),
                checkbox("Info", self.state.log_filter_info)
                    .on_toggle(|_| Message::ToggleLogFilterInfo),
            ]
            .align_y(alignment::Vertical::Center)
            .spacing(2),
        )
        .padding(4)
        .style(container::rounded_box);

        let status_bar = container(
            row![
                button("Clear").on_press(Message::ClearLogs),
                Space::with_width(5),
                button("Copy").on_press(Message::CopyLogs),
                Space::with_width(Length::Fill),
                text(format!(
                    "Proxy: {}:{} | Admin: {}:{}",
                    if self.state.proxy_localhost_only { "127.0.0.1" } else { "0.0.0.0" },
                    self.state.proxy_port,
                    if self.state.admin_localhost_only { "127.0.0.1" } else { "0.0.0.0" },
                    self.state.admin_port
                ))
                .size(12),
            ]
            .align_y(alignment::Vertical::Center)
            .spacing(5),
        )
        .padding(6)
        .style(container::rounded_box);

        column![
            toolbar,
            log_filter_bar,
            container(log_display)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(4),
            status_bar,
        ]
        .into()
    }

    fn view_settings(&self) -> Element<'_, Message> {
        let content = column![
            text("Settings").size(22),
            Space::with_height(20),
            row![
                text("Proxy Port:").width(120),
                text_input("", &self.state.settings_proxy_port)
                    .on_input(Message::SettingsProxyPortChanged)
                    .width(200),
            ]
            .spacing(10),
            Space::with_height(8),
            row![
                text("Admin Port:").width(120),
                text_input("", &self.state.settings_admin_port)
                    .on_input(Message::SettingsAdminPortChanged)
                    .width(200),
            ]
            .spacing(10),
            Space::with_height(8),
            row![
                text("Proxy Localhost Only:").width(160),
                checkbox("", self.state.settings_localhost_only)
                    .on_toggle(Message::SettingsLocalhostOnlyChanged),
            ]
            .spacing(10),
            Space::with_height(8),
            row![
                text("Admin Localhost Only:").width(160),
                checkbox("", self.state.settings_admin_localhost_only)
                    .on_toggle(Message::SettingsAdminLocalhostOnlyChanged),
            ]
            .spacing(10),
            Space::with_height(15),
            if !self.state.settings_error.is_empty() {
                column![
                    text(&self.state.settings_error).color(Color::from_rgb(0.94, 0.27, 0.27)),
                    Space::with_height(10),
                ]
            } else {
                column![]
            },
            row![
                button("Save").on_press(Message::SaveSettings),
                Space::with_width(10),
                button("Cancel").on_press(Message::CancelSettings),
            ]
            .spacing(5),
        ]
        .padding(20)
        .align_x(alignment::Horizontal::Center);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn view_setup_password(&self) -> Element<'_, Message> {
        let content = column![
            text("First Time Setup").size(22),
            Space::with_height(15),
            text("First time startup detected. Please set the admin password."),
            text("Username: admin").color(Color::from_rgb(0.58, 0.64, 0.72)),
            Space::with_height(20),
            row![
                text("Password (min 6 chars):").width(160),
                text_input("", &self.state.setup_password)
                    .on_input(Message::SetupPasswordChanged)
                    .secure(true)
                    .width(200),
            ]
            .spacing(10),
            Space::with_height(8),
            row![
                text("Confirm Password:").width(160),
                text_input("", &self.state.setup_confirm_password)
                    .on_input(Message::SetupConfirmPasswordChanged)
                    .secure(true)
                    .width(200),
            ]
            .spacing(10),
            Space::with_height(15),
            if !self.state.setup_error.is_empty() {
                column![
                    text(&self.state.setup_error).color(Color::from_rgb(0.94, 0.27, 0.27)),
                    Space::with_height(10),
                ]
            } else {
                column![]
            },
            button("Confirm").on_press(Message::ConfirmSetupPassword),
        ]
        .padding(20)
        .align_x(alignment::Horizontal::Center);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

#[derive(Debug)]
struct AppConfig {
    proxy_port: u16,
    admin_port: u16,
    proxy_localhost_only: bool,
    admin_localhost_only: bool,
}

fn load_configuration() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let config_path = modelproxy::config::Config::get_save_path();
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        let config: serde_json::Value = serde_json::from_str(&content)?;
        let proxy_host = config["server"]["host"].as_str().unwrap_or("0.0.0.0");
        let admin_host = config["admin"]["host"].as_str().unwrap_or("127.0.0.1");
        Ok(AppConfig {
            proxy_port: config["server"]["port"].as_u64().unwrap_or(3000) as u16,
            admin_port: config["admin"]["port"].as_u64().unwrap_or(3001) as u16,
            proxy_localhost_only: proxy_host == "127.0.0.1",
            admin_localhost_only: admin_host == "127.0.0.1",
        })
    } else {
        Ok(AppConfig {
            proxy_port: 3000,
            admin_port: 3001,
            proxy_localhost_only: false,
            admin_localhost_only: true,
        })
    }
}

fn save_configuration(proxy_port: u16, admin_port: u16, proxy_host: &str, admin_host: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = modelproxy::config::Config::get_save_path();

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content)?
    } else {
        let jwt_secret = modelproxy::utils::secrets::generate_secure_secret();
        let upstream_key_secret = modelproxy::utils::secrets::generate_secure_secret();
        serde_json::json!({
            "server": {
                "host": "0.0.0.0",
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
                "secret": jwt_secret,
                "expiration_hours": 0
            },
            "upstream_key": {
                "secret": upstream_key_secret
            },
            "audit_log": {
                "enabled": true,
                "path": "data/audit_logs"
            }
        })
    };

    config["server"]["host"] = serde_json::Value::String(proxy_host.to_string());
    config["server"]["port"] = serde_json::Value::Number(proxy_port.into());
    config["admin"]["host"] = serde_json::Value::String(admin_host.to_string());
    config["admin"]["port"] = serde_json::Value::Number(admin_port.into());

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

async fn run_service(
    shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    log_tx: std::sync::mpsc::Sender<LogEntry>,
    proxy_port: u16,
    admin_port: u16,
    proxy_localhost_only: bool,
    admin_localhost_only: bool,
    password_request: Arc<Mutex<Option<std::sync::mpsc::SyncSender<String>>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::Router;
    use axum::routing::{delete, get, post, put};
    use tower_http::cors::{Any, CorsLayer};
    use tower_http::compression::CompressionLayer;
    use tower_http::trace::TraceLayer;

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Loading configuration...".to_string() });

    let config_path = modelproxy::config::Config::get_save_path();
    let mut config = if config_path.exists() {
        modelproxy::config::Config::load()?
    } else {
        let _ = log_tx.send(LogEntry { level: LogLevel::Warn, message: "No config file found, creating default...".to_string() });
        let proxy_host = if proxy_localhost_only { "127.0.0.1" } else { "0.0.0.0" };
        let admin_host = if admin_localhost_only { "127.0.0.1" } else { "0.0.0.0" };
        save_configuration(proxy_port, admin_port, proxy_host, admin_host)?;
        modelproxy::config::Config::load()?
    };

    if modelproxy::utils::secrets::is_insecure_default_jwt_secret(&config.jwt.secret) {
        let new_secret = modelproxy::utils::secrets::generate_secure_secret();
        config.jwt.secret = new_secret;
        if config.jwt.upstream_key_secret.is_none() {
            config.jwt.upstream_key_secret = Some(modelproxy::utils::secrets::generate_secure_secret());
        }
        let save_path = modelproxy::config::Config::get_save_path();
        config.save_to_file(save_path.to_str().unwrap())?;
    }

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Initializing database...".to_string() });
    let db_path = config.database.get_db_path();
    let pool = modelproxy::db::create_pool(&db_path).await?;
    modelproxy::db::run_migrations(&pool).await?;

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Checking admin user...".to_string() });
    let admin_exists = modelproxy::db::users::admin_exists(&pool).await?;
    if !admin_exists {
        let _ = log_tx.send(LogEntry { level: LogLevel::Warn, message: "Admin user not found, requesting password setup...".to_string() });
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        {
            let mut req = password_request.lock().unwrap();
            *req = Some(tx);
        }
        let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Waiting for password setup via GUI...".to_string() });
        match rx.recv_timeout(std::time::Duration::from_secs(300)) {
            Ok(password) => {
                let password_hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                    .map_err(|e| format!("Password hash error: {}", e))?;
                modelproxy::db::users::create_admin(&pool, "admin", &password_hash, "admin@localhost").await?;
                let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Admin user created successfully".to_string() });
            }
            Err(_) => {
                let _ = log_tx.send(LogEntry { level: LogLevel::Error, message: "Password setup timed out".to_string() });
                return Err("Password setup timed out".into());
            }
        }
    }

    let proxy_log_path = config.audit_log.get_resolved_path();
    let store_manager = Arc::new(
        modelproxy::store::StoreManager::new(pool.clone())
            .with_audit_log_writer(&config.audit_log.path)
            .map_err(|e| format!("Audit log init error: {}", e))?
            .with_proxy_log_writer(&proxy_log_path)
            .map_err(|e| format!("Proxy log init error: {}", e))?,
    );
    store_manager.init_from_sqlite(&pool).await?;

    let jwt_service = Arc::new(modelproxy::auth::JwtService::new(&config.jwt));

    let client = modelproxy::proxy::UpstreamClient::new(&config.proxy)
        .map_err(|e| format!("Upstream client error: {}", e))?;
    let client_arc = Arc::new(client.clone());
    let rate_limiter = Arc::new(modelproxy::proxy::RateLimiter::new(
        config.rate_limit.window_size_secs,
        config.rate_limit.cleanup_interval_secs,
    ));
    let upstream_rate_limiter = Arc::new(modelproxy::proxy::UpstreamRateLimiter::new());
    let load_balancer = modelproxy::proxy::LoadBalancer::new();
    let blocker = modelproxy::proxy::UpstreamBlocker::new(Some(600));
    let error_logger = modelproxy::proxy::UpstreamErrorLogger::new(
        &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    ).map_err(|e| format!("Error logger init error: {}", e))?;

    let proxy_state = modelproxy::proxy::ProxyState {
        store: store_manager.clone(),
        client,
        rate_limiter,
        upstream_rate_limiter: upstream_rate_limiter.clone(),
        load_balancer,
        config: config.proxy.clone(),
        blocker,
        error_logger: Some(error_logger),
    };

    let proxy_routes = Router::new()
        .route("/models", get(modelproxy::proxy::handlers::list_models))
        .route("/chat/completions", post(modelproxy::proxy::handlers::proxy_handler))
        .route("/completions", post(modelproxy::proxy::handlers::proxy_handler))
        .route("/messages", post(modelproxy::proxy::handlers::proxy_handler))
        .with_state(proxy_state);

    let proxy_app = Router::new()
        .nest("/v1", proxy_routes)
        .layer(CompressionLayer::new())
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http());

    let public_auth_routes = {
        let routes = Router::new().route("/login", post(modelproxy::auth::handlers::login));
        let routes = if config.admin.allow_public_registration {
            routes.route("/register", post(modelproxy::auth::handlers::register))
        } else {
            routes
        };
        routes.with_state((pool.clone(), jwt_service.clone(), store_manager.clone()))
    };

    let protected_auth_routes = Router::new()
        .route("/logout", post(modelproxy::auth::handlers::logout))
        .route("/me", get(modelproxy::auth::handlers::get_current_user))
        .route("/change-password", post(modelproxy::auth::handlers::change_password))
        .with_state((pool.clone(), jwt_service.clone(), store_manager.clone()));

    let admin_state = modelproxy::store::AdminState::new(
        pool.clone(),
        store_manager.clone(),
        upstream_rate_limiter.clone(),
    );

    let api_key_routes = Router::new()
        .route("/", get(modelproxy::api_keys::list_my_keys).post(modelproxy::api_keys::create_key))
        .route("/:id", get(modelproxy::api_keys::get_key).put(modelproxy::api_keys::update_key).delete(modelproxy::api_keys::delete_key))
        .route("/all", get(modelproxy::api_keys::list_all_keys))
        .with_state(admin_state.clone());

    let usage_routes = Router::new()
        .route("/me", get(modelproxy::usage::get_my_usage))
        .route("/me/quota", get(modelproxy::usage::get_my_quota))
        .route("/all", get(modelproxy::usage::get_all_usage))
        .route("/user", get(modelproxy::usage::get_user_usage))
        .with_state(admin_state.clone());

    let audit_routes = Router::new()
        .route("/proxy", get(modelproxy::audit::list_proxy_audit_logs))
        .route("/proxy/export", get(modelproxy::audit::export_proxy_audit_logs))
        .route("/proxy/:id", get(modelproxy::audit::get_proxy_audit_log))
        .route("/", get(modelproxy::audit::list_audit_logs))
        .route("/:id", get(modelproxy::audit::get_audit_log))
        .with_state(admin_state.clone());

    let user_admin_routes = Router::new()
        .route("/", get(modelproxy::admin::list_users).post(modelproxy::admin::create_user))
        .route("/:id", get(modelproxy::admin::get_user).put(modelproxy::admin::update_user).delete(modelproxy::admin::delete_user))
        .route("/:id/reset-password", post(modelproxy::admin::reset_user_password))
        .with_state(admin_state.clone());

    let upstream_admin_routes = Router::new()
        .route("/", get(modelproxy::admin::list_upstreams).post(modelproxy::admin::create_upstream))
        .route("/:id", get(modelproxy::admin::get_upstream).put(modelproxy::admin::update_upstream).delete(modelproxy::admin::delete_upstream))
        .route("/:id/test", post(modelproxy::admin::test_upstream))
        .route("/:id/test-model", post(modelproxy::admin::test_upstream_model))
        .route("/:id/status", put(modelproxy::admin::update_upstream_status))
        .route("/:id/api-key", get(modelproxy::admin::get_upstream_api_key))
        .route("/groups", get(modelproxy::admin::list_upstream_groups).post(modelproxy::admin::create_upstream_group))
        .route("/groups/:id", delete(modelproxy::admin::delete_upstream_group))
        .with_state(admin_state.clone());

    let model_admin_routes = Router::new()
        .route("/", get(modelproxy::admin::list_models))
        .route("/fetch", get(modelproxy::admin::fetch_upstream_models))
        .route("/my", get(modelproxy::admin::get_user_models))
        .route("/conditional-aliases", get(modelproxy::admin::list_conditional_aliases))
        .route("/conditional-aliases/:alias", put(modelproxy::admin::set_conditional_alias).delete(modelproxy::admin::delete_conditional_alias))
        .route("/conditional-aliases/:alias/visibility", put(modelproxy::admin::set_conditional_alias_visibility))
        .route("/refresh", post(modelproxy::admin::refresh_cache))
        .route("/:upstream_id/:model_name", put(modelproxy::admin::set_visibility))
        .with_state((pool.clone(), client_arc.clone(), store_manager.clone()));

    let settings_routes = Router::new()
        .route("/", get(modelproxy::admin::get_settings).put(modelproxy::admin::update_settings))
        .with_state(admin_state.clone());

    let public_settings_routes = Router::new()
        .route("/public", get(modelproxy::admin::get_public_settings))
        .with_state(admin_state.clone());

    let protected_routes = Router::new()
        .nest("/keys", api_key_routes)
        .nest("/usage", usage_routes)
        .nest("/audit", audit_routes)
        .nest("/users", user_admin_routes)
        .nest("/upstreams", upstream_admin_routes)
        .nest("/models", model_admin_routes)
        .nest("/settings", settings_routes)
        .layer(axum::middleware::from_fn_with_state(
            (jwt_service.clone(), store_manager.clone()),
            modelproxy::auth::middleware::auth_middleware,
        ));

    let admin_app = Router::new()
        .nest("/auth", public_auth_routes)
        .nest("/api/settings", public_settings_routes)
        .nest("/api", protected_routes.clone())
        .nest("/api/auth", protected_auth_routes.layer(axum::middleware::from_fn_with_state(
            (jwt_service.clone(), store_manager.clone()),
            modelproxy::auth::middleware::auth_middleware,
        )))
        .route("/static/*path", get(serve_embedded_static))
        .route("/", get(serve_embedded_index))
        .fallback(get(serve_embedded_fallback))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http());

    let admin_host = config.admin.host.clone();
    let admin_addr = format!("{}:{}", admin_host, admin_port);
    let proxy_host = config.server.host.clone();
    let proxy_addr = format!("{}:{}", proxy_host, proxy_port);

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: format!("Starting admin server on {}...", admin_addr) });
    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: format!("Starting proxy server on {}...", proxy_addr) });

    let admin_listener = tokio::net::TcpListener::bind(&admin_addr).await?;
    let proxy_listener = tokio::net::TcpListener::bind(&proxy_addr).await?;

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Servers started successfully".to_string() });

    let mut shutdown_rx = shutdown_rx;
    let shutdown_signal = async move {
        let _ = shutdown_rx.recv().await;
    };

    let _ = log_tx.send(LogEntry { level: LogLevel::Info, message: "Service is running".to_string() });

    let result = tokio::select! {
        r = axum::serve(admin_listener, admin_app).with_graceful_shutdown(shutdown_signal) => r,
        r = axum::serve(proxy_listener, proxy_app.into_make_service_with_connect_info::<std::net::SocketAddr>()) => r,
    };

    if let Err(e) = result {
        let _ = log_tx.send(LogEntry { level: LogLevel::Error, message: format!("Server error: {}", e) });
    }

    Ok(())
}

async fn serve_embedded_static(AxumPath(path): AxumPath<String>) -> Response {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gui_log_path = modelproxy::config::AuditLogConfig::default().get_resolved_path();
    let gui_log_sub_path = std::path::Path::new(&gui_log_path).join("gui");
    if let Ok(writer) = GuiLogFileWriter::new(gui_log_sub_path.to_str().unwrap_or("data/audit_logs/gui")) {
        let _ = GUI_LOG_WRITER.set(Mutex::new(writer));
    }

    let (log_tx, log_rx) = std::sync::mpsc::channel::<LogEntry>();
    GUI_LOG_SENDER.set(log_tx).ok();

    let gui_layer = GuiTracingLayer::new(GUI_LOG_SENDER.get().unwrap().clone());
    tracing_subscriber::registry()
        .with(gui_layer)
        .init();

    let settings = Settings {
        ..Default::default()
    };

    let window_settings = iced::window::Settings {
        size: iced::Size::new(800.0, 560.0),
        min_size: Some(iced::Size::new(600.0, 420.0)),
        ..Default::default()
    };

    iced::application(ModelProxy::title, ModelProxy::update, ModelProxy::view)
        .subscription(ModelProxy::subscription)
        .window(window_settings)
        .settings(settings)
        .run_with(|| {
            let mut app = ModelProxy::new(log_rx);
            app.state.start_service();
            (app, Task::none())
        })?;

    Ok(())
}
