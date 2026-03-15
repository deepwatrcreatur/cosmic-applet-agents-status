use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use cosmic::{
    Element, Task, app,
    app::Core,
    applet::padded_control,
    iced::{
        Alignment, Length, Subscription,
        futures::{SinkExt, channel::mpsc},
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        widget::{column, row},
        window::Id,
    },
    iced_futures::stream,
    theme,
    widget::{button, container, divider, icon, text},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::process::Command;

const APP_ID: &str = "com.deepwatrcreatur.CosmicAppletAgentsStatus";
const CONFIG_ENV: &str = "AGENTS_STATUS_CONFIG";
const DEFAULT_POLL_SECONDS: u64 = 90;

pub struct AgentsApplet {
    core: Core,
    popup: Option<Id>,
    state: AppState,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    Refresh,
    Refreshed(Result<AgentsSnapshot, String>),
    OpenDashboard(String),
}

#[derive(Debug, Clone, Default)]
struct AppState {
    snapshot: Option<AgentsSnapshot>,
    error: Option<String>,
    status_text: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    #[serde(default)]
    agents: Vec<AgentDefinition>,
    #[serde(default = "default_poll_seconds")]
    poll_seconds: u64,
    #[serde(default = "default_claude_cache_ttl")]
    claude_cache_ttl_seconds: u64,
    #[serde(default)]
    openrouter_api_key_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentDefinition {
    id: String,
    name: String,
    command: String,
    #[serde(default)]
    dashboard_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsSnapshot {
    generated_at: String,
    label: String,
    agents: Vec<AgentStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentStatus {
    id: String,
    name: String,
    command: String,
    installed: bool,
    state: AgentState,
    summary: String,
    details: Vec<String>,
    #[serde(default)]
    metrics: AgentMetrics,
    #[serde(default)]
    dashboard_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AgentState {
    Unknown,
    Missing,
    Installed,
    Ready,
    Warning,
    Error,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AgentMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    current_utilization: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekly_utilization: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekly_resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOAuth>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOAuth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
}

#[derive(Debug, Deserialize)]
struct UsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterKeyResponse {
    data: OpenRouterKeyData,
}

#[derive(Debug, Deserialize)]
struct OpenRouterKeyData {
    label: Option<String>,
    usage: Option<f64>,
    limit: Option<f64>,
    limit_remaining: Option<f64>,
    is_free_tier: Option<bool>,
    rate_limit: Option<OpenRouterRateLimit>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterRateLimit {
    requests: Option<i32>,  // Can be -1 for unlimited
    interval: Option<String>,
}

// Claude local stats cache structures
#[derive(Debug, Deserialize)]
struct ClaudeStatsCache {
    #[serde(rename = "dailyModelTokens")]
    daily_model_tokens: Option<Vec<DailyModelTokens>>,
    #[serde(rename = "modelUsage")]
    model_usage: Option<std::collections::HashMap<String, ModelUsage>>,
    #[serde(rename = "totalSessions")]
    total_sessions: Option<u32>,
    #[serde(rename = "totalMessages")]
    total_messages: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct DailyModelTokens {
    date: String,
    #[serde(rename = "tokensByModel")]
    tokens_by_model: std::collections::HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct ModelUsage {
    #[serde(rename = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(rename = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(rename = "cacheReadInputTokens")]
    cache_read_input_tokens: Option<u64>,
    #[serde(rename = "cacheCreationInputTokens")]
    cache_creation_input_tokens: Option<u64>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
}

// Copilot session event structures
#[derive(Debug, Deserialize)]
struct CopilotEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: Option<serde_json::Value>,
}

fn default_poll_seconds() -> u64 {
    DEFAULT_POLL_SECONDS
}

fn default_claude_cache_ttl() -> u64 {
    60
}

impl cosmic::Application for AgentsApplet {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        (
            Self {
                core,
                popup: None,
                state: AppState {
                    status_text: "AI ...".to_string(),
                    ..Default::default()
                },
            },
            cosmic::task::message(Message::Refresh),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::run(poll_subscription)
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                return if let Some(popup) = self.popup.take() {
                    destroy_popup(popup)
                } else {
                    let id = Id::unique();
                    self.popup = Some(id);
                    let settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        id,
                        None,
                        None,
                        None,
                    );
                    get_popup(settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::Refresh => {
                return Task::perform(refresh_snapshot(), |result| {
                    cosmic::Action::App(Message::Refreshed(result.map_err(|err| err.to_string())))
                });
            }
            Message::Refreshed(result) => match result {
                Ok(snapshot) => {
                    self.state.status_text = snapshot.label.clone();
                    self.state.snapshot = Some(snapshot);
                    self.state.error = None;
                }
                Err(err) => {
                    self.state.status_text = "AI err".to_string();
                    self.state.error = Some(err);
                    self.state.snapshot = None;
                }
            },
            Message::OpenDashboard(url) => {
                // Open URL in default browser
                let _ = std::process::Command::new("xdg-open")
                    .arg(&url)
                    .spawn();
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let icon_name = match (&self.state.snapshot, &self.state.error) {
            (_, Some(_)) => "dialog-error-symbolic",
            (Some(snapshot), None) => {
                let all_ready = snapshot.agents.iter().all(|a| {
                    matches!(a.state, AgentState::Ready | AgentState::Installed)
                });
                if all_ready {
                    "object-select-symbolic"
                } else {
                    "dialog-warning-symbolic"
                }
            }
            _ => "utilities-terminal-symbolic",
        };

        let content = row![
            icon::from_name(icon_name).size(16),
            text(self.state.status_text.as_str()),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        button::custom(content)
            .padding([0, self.core.applet.suggested_padding(true).0])
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let cosmic::cosmic_theme::Spacing {
            space_xxs, space_s, ..
        } = theme::active().cosmic().spacing;

        let header = if let Some(snapshot) = &self.state.snapshot {
            let ready_count = snapshot
                .agents
                .iter()
                .filter(|a| matches!(a.state, AgentState::Ready | AgentState::Installed))
                .count();
            column![
                text("Coding Agents").size(18),
                text(format!(
                    "{}/{} agents ready",
                    ready_count,
                    snapshot.agents.len()
                ))
                .size(14),
            ]
            .spacing(4)
        } else if let Some(err) = &self.state.error {
            column![
                text("Agents Status").size(18),
                text(err).size(14),
            ]
            .spacing(4)
        } else {
            column![text("Agents Status").size(18), text("Loading...").size(14)].spacing(4)
        };

        let mut content = column![container(header).padding([12, 16])];

        if let Some(snapshot) = &self.state.snapshot {
            for agent in &snapshot.agents {
                content = content.push(padded_control(divider::horizontal::default()).padding([
                    space_xxs, space_s,
                ]));

                let state_icon = match agent.state {
                    AgentState::Ready => "emblem-ok-symbolic",
                    AgentState::Installed => "object-select-symbolic",
                    AgentState::Warning => "dialog-warning-symbolic",
                    AgentState::Error => "dialog-error-symbolic",
                    AgentState::Missing => "action-unavailable-symbolic",
                    AgentState::Unknown => "dialog-question-symbolic",
                };

                // Agent name row with optional dashboard link
                let name_row = row![
                    icon::from_name(state_icon).size(14),
                    text(&agent.name).size(15),
                ]
                .spacing(8)
                .align_y(Alignment::Center);

                if let Some(url) = &agent.dashboard_url {
                    let agent_row = row![
                        name_row,
                        container(icon::from_name("emblem-web-symbolic").size(12))
                            .padding([0, 4]),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                    content = content.push(
                        button::custom(agent_row)
                            .on_press(Message::OpenDashboard(url.clone()))
                            .padding([4, 8])
                            .class(cosmic::theme::Button::Text),
                    );
                } else {
                    content = content.push(name_row);
                }

                content = content.push(text(&agent.summary).size(13));

                for detail in &agent.details {
                    content = content.push(text(format!("  {detail}")).size(12));
                }

                if let Some(resets_at) = &agent.metrics.current_resets_at {
                    if let Some(relative) = format_relative(resets_at) {
                        content = content.push(text(format!("  Resets in {relative}")).size(12));
                    }
                }
            }

            content = content.push(padded_control(divider::horizontal::default()).padding([
                space_xxs, space_s,
            ]));

            if let Ok(ts) = DateTime::parse_from_rfc3339(&snapshot.generated_at) {
                let local: DateTime<Local> = ts.into();
                content = content.push(
                    text(format!("Updated {}", local.format("%I:%M%p").to_string().to_lowercase()))
                        .size(12),
                );
            }
        } else {
            content = content.push(padded_control(divider::horizontal::default()).padding([
                space_xxs, space_s,
            ]));
            content = content.push(
                text("Create ~/.config/cosmic-applet-agents-status/config.toml").size(14),
            );
        }

        self.core
            .applet
            .popup_container(container(content.padding([8, 0])))
            .into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

fn format_relative(iso_str: &str) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(iso_str).ok()?;
    let now = Utc::now();
    let duration = parsed.signed_duration_since(now);
    let seconds = duration.num_seconds();

    if seconds <= 0 {
        return Some("now".to_string());
    }
    if seconds < 3600 {
        return Some(format!("{}m", seconds / 60));
    }
    if seconds < 86400 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        return Some(format!("{hours}h {minutes}m"));
    }
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    Some(format!("{days}d {hours}h"))
}

fn poll_subscription() -> impl cosmic::iced::futures::Stream<Item = Message> {
    stream::channel(1, move |mut output: mpsc::Sender<Message>| async move {
        loop {
            let _ = output.send(Message::Refresh).await;
            let period = Duration::from_secs(
                read_config()
                    .map(|config| config.poll_seconds.max(5))
                    .unwrap_or(DEFAULT_POLL_SECONDS),
            );
            tokio::time::sleep(period).await;
        }
    })
}

fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var(CONFIG_ENV) {
        return PathBuf::from(path);
    }

    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("cosmic-applet-agents-status");
    path.push("config.toml");
    path
}

fn read_config() -> Result<Config> {
    let path = config_path();
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    toml::from_str(&content).context("failed to parse config.toml")
}

async fn refresh_snapshot() -> Result<AgentsSnapshot> {
    let config = read_config().unwrap_or(Config {
        agents: vec![],
        poll_seconds: DEFAULT_POLL_SECONDS,
        claude_cache_ttl_seconds: 60,
        openrouter_api_key_path: None,
    });

    let mut agents = Vec::new();
    for def in &config.agents {
        let status = collect_agent_status(def, &config).await;
        agents.push(status);
    }

    let label = build_label(&agents);

    Ok(AgentsSnapshot {
        generated_at: Utc::now().to_rfc3339(),
        label,
        agents,
    })
}

fn build_label(agents: &[AgentStatus]) -> String {
    // If we have Claude with usage metrics, show that
    if let Some(claude) = agents.iter().find(|a| a.id == "claude") {
        if let Some(pct) = claude.metrics.current_utilization {
            return format!("AI {pct}%");
        }
    }

    // Otherwise show ready/total count
    let ready = agents
        .iter()
        .filter(|a| matches!(a.state, AgentState::Ready | AgentState::Installed))
        .count();
    format!("AI {}/{}", ready, agents.len())
}

async fn collect_agent_status(def: &AgentDefinition, config: &Config) -> AgentStatus {
    let installed = command_exists(&def.command).await;

    // Get dashboard URL from config or use default
    let dashboard_url = def.dashboard_url.clone().or_else(|| {
        match def.id.as_str() {
            "claude" => Some("https://console.anthropic.com/settings/plans".to_string()),
            "codex" => Some("https://platform.openai.com/usage".to_string()),
            "gemini" => Some("https://aistudio.google.com/".to_string()),
            "copilot" => Some("https://github.com/settings/copilot".to_string()),
            "openrouter" => Some("https://openrouter.ai/credits".to_string()),
            "opencode" | "opencode-zai" => Some("https://openrouter.ai/credits".to_string()),
            _ => None,
        }
    });

    let mut status = AgentStatus {
        id: def.id.clone(),
        name: def.name.clone(),
        command: def.command.clone(),
        installed,
        state: AgentState::Unknown,
        summary: "No status available".to_string(),
        details: vec![],
        metrics: AgentMetrics::default(),
        dashboard_url,
    };

    if !installed {
        status.state = AgentState::Missing;
        status.summary = format!("{} not found on PATH", def.command);
        return status;
    }

    // Dispatch to specific collector based on agent ID
    match def.id.as_str() {
        "claude" => collect_claude(&mut status, config).await,
        "codex" => collect_codex(&mut status).await,
        "gemini" => collect_gemini(&mut status).await,
        "copilot" => collect_copilot(&mut status).await,
        "openrouter" => collect_openrouter(&mut status, config).await,
        "opencode" | "opencode-zai" => collect_opencode(&mut status, &def.id).await,
        _ => {
            status.state = AgentState::Installed;
            status.summary = format!("{} installed", def.command);
        }
    }

    status
}

async fn command_exists(cmd: &str) -> bool {
    let result = Command::new("bash")
        .args(["-lc", &format!("command -v {cmd} >/dev/null 2>&1")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    matches!(result, Ok(status) if status.success())
}

async fn collect_claude(status: &mut AgentStatus, _config: &Config) {
    let home = dirs::home_dir().unwrap_or_default();
    let creds_path = home.join(".claude/.credentials.json");
    let stats_path = home.join(".claude/stats-cache.json");

    // Read local stats first (always available if Claude has been used)
    let local_stats: Option<ClaudeStatsCache> = fs::read_to_string(&stats_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok());

    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().or_else(|| {
        let content = fs::read_to_string(&creds_path).ok()?;
        let creds: ClaudeCredentials = serde_json::from_str(&content).ok()?;
        creds.claude_ai_oauth?.access_token
    });

    let Some(token) = token else {
        // Even without API access, show local stats if available
        if let Some(stats) = local_stats {
            status.state = AgentState::Warning;
            status.summary = "No OAuth token - showing local stats".to_string();
            add_claude_local_stats(status, &stats);
        } else {
            status.state = AgentState::Warning;
            status.summary = "Claude installed but no OAuth token found".to_string();
        }
        return;
    };

    // Fetch usage from API
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "cosmic-applet-agents-status/0.1")
        .send()
        .await;

    let Ok(resp) = response else {
        status.state = AgentState::Warning;
        status.summary = "Claude token found but usage API unreachable".to_string();
        if let Some(stats) = local_stats {
            add_claude_local_stats(status, &stats);
        }
        return;
    };

    // Check for rate limiting or other HTTP errors
    if resp.status() == 429 {
        status.state = AgentState::Ready;
        status.summary = "Rate limited - try again later".to_string();
        if let Some(stats) = local_stats {
            add_claude_local_stats(status, &stats);
        }
        return;
    }

    if !resp.status().is_success() {
        status.state = AgentState::Warning;
        status.summary = format!("API error: {}", resp.status());
        if let Some(stats) = local_stats {
            add_claude_local_stats(status, &stats);
        }
        return;
    }

    let Ok(usage) = resp.json::<ClaudeUsageResponse>().await else {
        status.state = AgentState::Warning;
        status.summary = "Failed to parse usage response".to_string();
        if let Some(stats) = local_stats {
            add_claude_local_stats(status, &stats);
        }
        return;
    };

    let current_pct = usage
        .five_hour
        .as_ref()
        .and_then(|w| w.utilization)
        .map(|u| (u * 100.0).round() as u32)
        .unwrap_or(0);

    let weekly_pct = usage
        .seven_day
        .as_ref()
        .and_then(|w| w.utilization)
        .map(|u| (u * 100.0).round() as u32)
        .unwrap_or(0);

    status.state = AgentState::Ready;
    status.summary = format!("Current {current_pct}% | Weekly {weekly_pct}%");

    if let Some(five_hour) = &usage.five_hour {
        if let Some(resets_at) = &five_hour.resets_at {
            let relative = format_relative(resets_at).unwrap_or_else(|| "unknown".to_string());
            status.details.push(format!("Current window: {current_pct}% used, resets in {relative}"));
            status.metrics.current_resets_at = Some(resets_at.clone());
        }
    }
    if let Some(seven_day) = &usage.seven_day {
        if let Some(resets_at) = &seven_day.resets_at {
            let relative = format_relative(resets_at).unwrap_or_else(|| "unknown".to_string());
            status.details.push(format!("Weekly window: {weekly_pct}% used, resets in {relative}"));
            status.metrics.weekly_resets_at = Some(resets_at.clone());
        }
    }

    status.metrics.current_utilization = Some(current_pct);
    status.metrics.weekly_utilization = Some(weekly_pct);

    // Add local stats as additional details
    if let Some(stats) = local_stats {
        add_claude_local_stats(status, &stats);
    }
}

fn add_claude_local_stats(status: &mut AgentStatus, stats: &ClaudeStatsCache) {
    // Total tokens across all models
    if let Some(model_usage) = &stats.model_usage {
        let total_input: u64 = model_usage.values()
            .filter_map(|m| m.input_tokens)
            .sum();
        let total_output: u64 = model_usage.values()
            .filter_map(|m| m.output_tokens)
            .sum();
        let total_cache_read: u64 = model_usage.values()
            .filter_map(|m| m.cache_read_input_tokens)
            .sum();

        if total_input > 0 || total_output > 0 {
            status.details.push(format!(
                "Local: {}K in / {}K out",
                total_input / 1000,
                total_output / 1000
            ));
        }
        if total_cache_read > 0 {
            status.details.push(format!("Cache reads: {}M tokens", total_cache_read / 1_000_000));
        }
    }

    if let (Some(sessions), Some(messages)) = (stats.total_sessions, stats.total_messages) {
        status.details.push(format!("Sessions: {sessions} | Messages: {messages}"));
    }
}

async fn collect_codex(status: &mut AgentStatus) {
    let home = dirs::home_dir().unwrap_or_default();
    let codex_dir = home.join(".codex");

    // Try to get login status
    let login_output = Command::new("codex")
        .args(["login", "status"])
        .output()
        .await;

    let summary = match login_output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            // Prefer stdout, fall back to stderr
            if stdout.is_empty() { stderr } else { stdout }
        }
        Ok(output) => {
            // Command ran but failed, check stderr
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        }
        _ => "Unable to read login status".to_string(),
    };

    // Query SQLite for usage stats
    let state_db = codex_dir.join("state_5.sqlite");
    let mut thread_count: Option<i64> = None;
    let mut total_tokens: Option<i64> = None;
    let mut today_tokens: Option<i64> = None;
    let mut week_tokens: Option<i64> = None;

    if state_db.exists() {
        if let Ok(conn) = rusqlite::Connection::open(&state_db) {
            // Total thread count
            thread_count = conn
                .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get::<_, i64>(0))
                .ok();

            // Total tokens across all time
            total_tokens = conn
                .query_row("SELECT SUM(tokens_used) FROM threads", [], |row| row.get::<_, i64>(0))
                .ok();

            // Tokens from last 24 hours (timestamps are in milliseconds)
            let day_ago_ms = (chrono::Utc::now().timestamp() - 86400) * 1000;
            today_tokens = conn
                .query_row(
                    "SELECT SUM(tokens_used) FROM threads WHERE updated_at > ?1",
                    [day_ago_ms],
                    |row| row.get::<_, i64>(0),
                )
                .ok();

            // Tokens from last 7 days
            let week_ago_ms = (chrono::Utc::now().timestamp() - 7 * 86400) * 1000;
            week_tokens = conn
                .query_row(
                    "SELECT SUM(tokens_used) FROM threads WHERE updated_at > ?1",
                    [week_ago_ms],
                    |row| row.get::<_, i64>(0),
                )
                .ok();
        }
    }

    status.state = if summary.to_lowercase().contains("logged in") {
        AgentState::Ready
    } else {
        AgentState::Warning
    };

    // Build enhanced summary with token counts
    if let Some(week) = week_tokens {
        if week > 0 {
            // Estimate cost at ~$0.01 per 1K tokens (rough average for GPT-4o)
            let estimated_cost = week as f64 / 1000.0 * 0.01;
            status.summary = format!(
                "{} | ~{}K tokens/week (~${:.2})",
                if summary.to_lowercase().contains("logged in") { "Logged in" } else { &summary },
                week / 1000,
                estimated_cost
            );
        } else {
            status.summary = summary;
        }
    } else {
        status.summary = summary;
    }

    if let Some(count) = thread_count {
        status.details.push(format!("Sessions: {count}"));
    }

    if let Some(today) = today_tokens {
        if today > 0 {
            status.details.push(format!("Today: {}K tokens", today / 1000));
        }
    }

    if let Some(total) = total_tokens {
        if total > 0 {
            status.details.push(format!("All time: {}K tokens", total / 1000));
        }
    }
}

async fn collect_gemini(status: &mut AgentStatus) {
    let home = dirs::home_dir().unwrap_or_default();
    let accounts_path = home.join(".gemini/google_accounts.json");

    let active_account: Option<String> = fs::read_to_string(&accounts_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|v| v.get("active")?.as_str().map(String::from));

    status.state = if active_account.is_some() {
        AgentState::Ready
    } else {
        AgentState::Installed
    };

    status.summary = if let Some(ref account) = active_account {
        format!("Signed in as {account}")
    } else {
        "Installed".to_string()
    };

    if let Some(account) = active_account {
        status.details.push(format!("Active account: {account}"));
    }
}

async fn collect_copilot(status: &mut AgentStatus) {
    let home = dirs::home_dir().unwrap_or_default();
    let copilot_dir = home.join(".copilot");
    let config_path = copilot_dir.join("config.json");

    let user: Option<String> = fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|v| v.get("last_logged_in_user")?.as_str().map(String::from));

    let session_dir = copilot_dir.join("session-state");

    // Parse session files for detailed stats
    let mut session_count = 0;
    let mut total_messages = 0;
    let mut max_tokens_in_session = 0u64;

    if let Ok(entries) = fs::read_dir(&session_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().map(|ext| ext == "jsonl").unwrap_or(false) {
                session_count += 1;

                // Parse JSONL to get token counts
                if let Ok(content) = fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(event) = serde_json::from_str::<CopilotEvent>(line) {
                            match event.event_type.as_str() {
                                "user.message" => total_messages += 1,
                                "session.truncation" => {
                                    // Extract token count from truncation event
                                    if let Some(data) = &event.data {
                                        if let Some(tokens) = data.get("preTruncationTokensInMessages")
                                            .and_then(|v| v.as_u64())
                                        {
                                            if tokens > max_tokens_in_session {
                                                max_tokens_in_session = tokens;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    status.state = if user.is_some() {
        AgentState::Ready
    } else {
        AgentState::Installed
    };

    // Build summary with token info
    if max_tokens_in_session > 0 {
        status.summary = format!(
            "{} | {}K tokens max",
            if user.is_some() { "Signed in" } else { "Installed" },
            max_tokens_in_session / 1000
        );
    } else {
        status.summary = format!(
            "{} | {} sessions",
            if user.is_some() { "Signed in" } else { "Installed" },
            session_count
        );
    }

    if let Some(u) = user {
        status.details.push(format!("User: {u}"));
    }
    status.details.push(format!("Sessions: {session_count} | Messages: {total_messages}"));

    if max_tokens_in_session > 0 {
        status.details.push(format!("Peak context: {}K tokens", max_tokens_in_session / 1000));
    }
}

async fn collect_openrouter(status: &mut AgentStatus, config: &Config) {
    // Read API key from configured path or default agenix location
    let api_key_path = config
        .openrouter_api_key_path
        .as_deref()
        .unwrap_or("/run/agenix/openrouter-api-key");

    let api_key = match fs::read_to_string(api_key_path) {
        Ok(key) => key.trim().to_string(),
        Err(_) => {
            status.state = AgentState::Warning;
            status.summary = "OpenRouter API key not found".to_string();
            status.details.push(format!("Expected at: {api_key_path}"));
            return;
        }
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = client
        .get("https://openrouter.ai/api/v1/auth/key")
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await;

    let Ok(resp) = response else {
        status.state = AgentState::Warning;
        status.summary = "OpenRouter API unreachable".to_string();
        return;
    };

    if !resp.status().is_success() {
        status.state = AgentState::Warning;
        status.summary = format!("API error: {}", resp.status());
        return;
    }

    let Ok(key_info) = resp.json::<OpenRouterKeyResponse>().await else {
        status.state = AgentState::Warning;
        status.summary = "Failed to parse OpenRouter response".to_string();
        return;
    };

    let data = &key_info.data;
    let usage = data.usage.unwrap_or(0.0);
    let limit = data.limit.unwrap_or(0.0);
    let remaining = data.limit_remaining.unwrap_or(limit - usage);

    status.state = AgentState::Ready;

    if limit > 0.0 {
        let pct_used = (usage / limit * 100.0).round() as u32;
        status.summary = format!("${:.2} remaining ({pct_used}% used)", remaining);
        status.details.push(format!("Used: ${:.2} / ${:.2}", usage, limit));
    } else if data.is_free_tier.unwrap_or(false) {
        status.summary = format!("Free tier - ${:.4} used", usage);
    } else {
        status.summary = format!("${:.2} used (no limit)", usage);
    }

    if let Some(rate_limit) = &data.rate_limit {
        if let (Some(requests), Some(interval)) = (rate_limit.requests, &rate_limit.interval) {
            if requests > 0 {
                status.details.push(format!("Rate limit: {} req/{}", requests, interval));
            }
        }
    }
}

async fn collect_opencode(status: &mut AgentStatus, agent_id: &str) {
    let home = dirs::home_dir().unwrap_or_default();
    let opencode_dir = home.join(".local/share/opencode");
    let db_path = opencode_dir.join("opencode-stable.db");
    let auth_path = opencode_dir.join("auth.json");

    // Check authentication status
    let authenticated = fs::read_to_string(&auth_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .map(|v| v.get("token").is_some() || v.get("access_token").is_some())
        .unwrap_or(false);

    let mut session_count: Option<i64> = None;
    let mut project_count: Option<i64> = None;
    let mut message_count: Option<i64> = None;

    if db_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            session_count = conn
                .query_row("SELECT COUNT(*) FROM session", [], |row| row.get::<_, i64>(0))
                .ok();

            project_count = conn
                .query_row("SELECT COUNT(*) FROM project", [], |row| row.get::<_, i64>(0))
                .ok();

            message_count = conn
                .query_row("SELECT COUNT(*) FROM message", [], |row| row.get::<_, i64>(0))
                .ok();
        }
    }

    let variant = if agent_id == "opencode-zai" { "Z.ai" } else { "Zen" };

    status.state = if authenticated || session_count.unwrap_or(0) > 0 {
        AgentState::Ready
    } else {
        AgentState::Installed
    };

    // Build summary
    if let Some(sessions) = session_count {
        if sessions > 0 {
            status.summary = format!("OpenCode {} | {} sessions", variant, sessions);
        } else {
            status.summary = format!("OpenCode {} installed", variant);
        }
    } else {
        status.summary = format!("OpenCode {} installed", variant);
    }

    if let Some(projects) = project_count {
        if projects > 0 {
            status.details.push(format!("Projects: {projects}"));
        }
    }

    if let Some(messages) = message_count {
        if messages > 0 {
            status.details.push(format!("Messages: {messages}"));
        }
    }

    if authenticated {
        status.details.push("Authenticated".to_string());
    }
}
