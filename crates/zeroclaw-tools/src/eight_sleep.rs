//! 8Sleep Pod integration tool — temperature, priming, alarms & sleep metrics.
//!
//! Controls an 8Sleep Pod via the cloud API (`client-api.8slp.net`, JWT auth).
//! Per-side selector (`left`/`right`) on relevant operations.
//!
//! Disclaimer: 8Sleep does not publish a stable public API. This integration
//! uses the same HTTPS endpoints the official mobile app calls and may break
//! without notice.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroclaw_api::tool::{Tool, ToolResult};
use zeroclaw_config::policy::{SecurityPolicy, ToolOperation};

const API_BASE: &str = "https://client-api.8slp.net/v1";
const MAX_ERROR_BODY_CHARS: usize = 500;

/// In-memory JWT cache — no on-disk persistence in v1.
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now >= self.expires_at.saturating_sub(60)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginResponse {
    session: Option<SessionInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionInfo {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expirationDate")]
    expiration_date: Option<u64>,
}

/// Tool for interacting with the 8Sleep Pod API — get bed state, metrics,
/// set temperature, priming, and alarms. Each action is gated by the
/// appropriate security operation (Read for queries, Act for mutations).
pub struct EightSleepTool {
    email: String,
    password: String,
    device_id: Option<String>,
    timeout: Duration,
    http: reqwest::Client,
    security: Arc<SecurityPolicy>,
    token: RwLock<Option<CachedToken>>,
    api_base: String,
}

impl EightSleepTool {
    /// Create a new 8Sleep tool with credentials, optional device ID, and
    /// security policy.
    pub fn new(
        email: String,
        password: String,
        device_id: Option<String>,
        timeout_secs: u64,
        security: Arc<SecurityPolicy>,
    ) -> Self {
        Self {
            email,
            password,
            device_id,
            timeout: Duration::from_secs(timeout_secs.max(5)),
            http: reqwest::Client::new(),
            security,
            token: RwLock::new(None),
            api_base: API_BASE.to_string(),
        }
    }

    /// Create with a custom API base URL (for testing with mock servers).
    #[cfg(test)]
    fn with_api_base(mut self, base: String) -> Self {
        self.api_base = base;
        self
    }

    fn bearer_header(token: &str) -> reqwest::header::HeaderValue {
        format!("Bearer {token}")
            .parse()
            .expect("valid bearer header")
    }

    /// Authenticate against the 8Sleep API and cache the JWT.
    async fn authenticate(&self) -> anyhow::Result<String> {
        {
            let guard = self.token.read();
            if let Some(ref t) = *guard
                && !t.is_expired()
            {
                return Ok(t.access_token.clone());
            }
        }

        let url = format!("{}/users/login", self.api_base);
        let body = json!({
            "email": self.email,
            "password": self.password,
        });
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(self.timeout)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated =
                crate::util_helpers::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("8Sleep login failed ({status}): {truncated}");
        }

        let login: LoginResponse = resp.json().await.map_err(|e| {
            ::zeroclaw_log::record!(
                ERROR,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                    .with_attrs(::serde_json::json!({"error": format!("{}", e)})),
                "eight_sleep: failed to parse login response"
            );
            anyhow::Error::msg(format!("8Sleep login response parse error: {e}"))
        })?;
        let session = login.session.ok_or_else(|| {
            ::zeroclaw_log::record!(
                ERROR,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure),
                "eight_sleep: login returned no session"
            );
            anyhow::Error::msg("8Sleep login returned no session")
        })?;
        let access_token = session.access_token.ok_or_else(|| {
            ::zeroclaw_log::record!(
                ERROR,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure),
                "eight_sleep: login returned no access token"
            );
            anyhow::Error::msg("8Sleep login returned no access token")
        })?;

        let expires_at = session.expiration_date.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + 86400
        });

        {
            let mut guard = self.token.write();
            *guard = Some(CachedToken {
                access_token: access_token.clone(),
                expires_at,
            });
        }

        Ok(access_token)
    }

    /// Invalidate cached token (used on 401 to force re-auth).
    fn invalidate_token(&self) {
        let mut guard = self.token.write();
        *guard = None;
    }

    /// Resolve the device ID — use configured value or auto-detect.
    async fn resolve_device_id(&self, token: &str) -> anyhow::Result<String> {
        if let Some(ref id) = self.device_id
            && !id.trim().is_empty()
        {
            return Ok(id.trim().to_string());
        }

        let url = format!("{}/users/me", self.api_base);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", Self::bearer_header(token))
            .timeout(self.timeout)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated =
                crate::util_helpers::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("8Sleep get user failed ({status}): {truncated}");
        }

        let user: serde_json::Value = resp.json().await?;
        let device_id = user["user"]["devices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|d| d["id"].as_str())
            .ok_or_else(|| {
                ::zeroclaw_log::record!(
                    ERROR,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                        .with_outcome(::zeroclaw_log::EventOutcome::Failure),
                    "eight_sleep: no devices found on account"
                );
                anyhow::Error::msg("No 8Sleep devices found on account")
            })?
            .to_string();

        Ok(device_id)
    }

    /// Get current bed state (temperature, priming status, alarm).
    async fn get_bed_state(&self) -> anyhow::Result<serde_json::Value> {
        let token = self.authenticate().await?;
        let device_id = self.resolve_device_id(&token).await?;
        let url = format!("{}/devices/{device_id}", self.api_base);

        let resp = self
            .http
            .get(&url)
            .header("Authorization", Self::bearer_header(&token))
            .timeout(self.timeout)
            .send()
            .await?;

        let status = resp.status();
        if status.as_u16() == 401 {
            drop(resp);
            self.invalidate_token();
            let token = self.authenticate().await?;
            let resp = self
                .http
                .get(&url)
                .header("Authorization", Self::bearer_header(&token))
                .timeout(self.timeout)
                .send()
                .await?;
            return Self::parse_response("get_bed_state", resp).await;
        }
        Self::parse_response("get_bed_state", resp).await
    }

    /// Get sleep metrics for a time range.
    async fn get_metrics(
        &self,
        side: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let token = self.authenticate().await?;
        let device_id = self.resolve_device_id(&token).await?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let from_ts = from
            .and_then(|f| f.parse::<u128>().ok())
            .unwrap_or(now_ms - 86_400_000);
        let to_ts = to.and_then(|t| t.parse::<u128>().ok()).unwrap_or(now_ms);

        let url = format!(
            "{}/devices/{device_id}/metrics?from={from_ts}&to={to_ts}&side={side}",
            self.api_base
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", Self::bearer_header(&token))
            .timeout(self.timeout)
            .send()
            .await?;

        let status = resp.status();
        if status.as_u16() == 401 {
            drop(resp);
            self.invalidate_token();
            let token = self.authenticate().await?;
            let resp = self
                .http
                .get(&url)
                .header("Authorization", Self::bearer_header(&token))
                .timeout(self.timeout)
                .send()
                .await?;
            return Self::parse_response("get_metrics", resp).await;
        }
        Self::parse_response("get_metrics", resp).await
    }

    /// Set bed temperature for a given side (range: -100 to 100).
    async fn set_temperature(
        &self,
        side: &str,
        temperature: i64,
    ) -> anyhow::Result<serde_json::Value> {
        let token = self.authenticate().await?;
        let device_id = self.resolve_device_id(&token).await?;
        let url = format!("{}/devices/{device_id}", self.api_base);

        let body = json!({
            "temperature": {
                side: {
                    "currentTarget": temperature
                }
            }
        });

        self.send_with_retry("set_temperature", &token, &url, |builder| {
            builder.json(&body)
        })
        .await
    }

    /// Set priming on/off for a given side.
    async fn set_priming(&self, side: &str, enabled: bool) -> anyhow::Result<serde_json::Value> {
        let token = self.authenticate().await?;
        let device_id = self.resolve_device_id(&token).await?;
        let url = format!("{}/devices/{device_id}", self.api_base);

        let body = json!({
            "priming": {
                side: {
                    "enabled": enabled
                }
            }
        });

        self.send_with_retry("set_priming", &token, &url, |builder| builder.json(&body))
            .await
    }

    /// Set alarm time for a given side.
    async fn set_alarm(
        &self,
        side: &str,
        time: &str,
        enabled: bool,
    ) -> anyhow::Result<serde_json::Value> {
        let token = self.authenticate().await?;
        let device_id = self.resolve_device_id(&token).await?;
        let url = format!("{}/devices/{device_id}", self.api_base);

        let body = json!({
            "alarm": {
                side: {
                    "enabled": enabled,
                    "time": time
                }
            }
        });

        self.send_with_retry("set_alarm", &token, &url, |builder| builder.json(&body))
            .await
    }

    /// Send a PUT request with 401 retry. Takes a closure that customizes
    /// the request builder with a body.
    async fn send_with_retry<F>(
        &self,
        label: &str,
        token: &str,
        url: &str,
        customize: F,
    ) -> anyhow::Result<serde_json::Value>
    where
        F: Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    {
        let resp = customize(
            self.http
                .put(url)
                .header("Authorization", Self::bearer_header(token))
                .header("Content-Type", "application/json")
                .timeout(self.timeout),
        )
        .send()
        .await?;

        let status = resp.status();
        if status.as_u16() == 401 {
            drop(resp);
            self.invalidate_token();
            let new_token = self.authenticate().await?;
            let retry_resp = customize(
                self.http
                    .put(url)
                    .header("Authorization", Self::bearer_header(&new_token))
                    .header("Content-Type", "application/json")
                    .timeout(self.timeout),
            )
            .send()
            .await?;
            return Self::parse_response(label, retry_resp).await;
        }
        Self::parse_response(label, resp).await
    }

    /// Parse an API response, returning a descriptive error on failure.
    async fn parse_response(
        label: &str,
        resp: reqwest::Response,
    ) -> anyhow::Result<serde_json::Value> {
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated =
                crate::util_helpers::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("8Sleep {label} failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }
}

#[async_trait]
impl Tool for EightSleepTool {
    fn name(&self) -> &str {
        "eight_sleep"
    }

    fn description(&self) -> &str {
        "Control an 8Sleep Pod: get bed state, sleep metrics, set temperature, priming, and alarms. \
         Unofficial API — may break without notice."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "get_bed_state",
                        "get_metrics",
                        "set_temperature",
                        "set_priming",
                        "set_alarm"
                    ],
                    "description": "The 8Sleep action to perform"
                },
                "side": {
                    "type": "string",
                    "enum": ["left", "right"],
                    "description": "Pod side (required for set_temperature, set_priming, set_alarm, get_metrics)"
                },
                "temperature": {
                    "type": "integer",
                    "minimum": -100,
                    "maximum": 100,
                    "description": "Target temperature for set_temperature (-100 to 100)"
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Enable/disable flag for set_priming and set_alarm"
                },
                "time": {
                    "type": "string",
                    "description": "Alarm time in HH:MM format for set_alarm"
                },
                "from": {
                    "type": "string",
                    "description": "Start timestamp (milliseconds) for get_metrics. Default: 24h ago"
                },
                "to": {
                    "type": "string",
                    "description": "End timestamp (milliseconds) for get_metrics. Default: now"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: action".into()),
                });
            }
        };

        let operation = match action {
            "get_bed_state" | "get_metrics" => ToolOperation::Read,
            "set_temperature" | "set_priming" | "set_alarm" => ToolOperation::Act,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Unknown action: {action}. Valid actions: get_bed_state, get_metrics, set_temperature, set_priming, set_alarm"
                    )),
                });
            }
        };

        if let Err(error) = self
            .security
            .enforce_tool_operation(operation, "eight_sleep")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let result = match action {
            "get_bed_state" => self.get_bed_state().await,
            "get_metrics" => {
                let side = args.get("side").and_then(|v| v.as_str()).unwrap_or("left");
                let from = args.get("from").and_then(|v| v.as_str());
                let to = args.get("to").and_then(|v| v.as_str());
                self.get_metrics(side, from, to).await
            }
            "set_temperature" => {
                let side = match args.get("side").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "set_temperature requires 'side' parameter (left or right)".into(),
                            ),
                        });
                    }
                };
                let temperature = match args.get("temperature").and_then(|v| v.as_i64()) {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "set_temperature requires 'temperature' parameter (-100 to 100)"
                                    .into(),
                            ),
                        });
                    }
                };
                self.set_temperature(side, temperature).await
            }
            "set_priming" => {
                let side = match args.get("side").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "set_priming requires 'side' parameter (left or right)".into(),
                            ),
                        });
                    }
                };
                let enabled = args
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.set_priming(side, enabled).await
            }
            "set_alarm" => {
                let side = match args.get("side").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "set_alarm requires 'side' parameter (left or right)".into(),
                            ),
                        });
                    }
                };
                let time = match args.get("time").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "set_alarm requires 'time' parameter (HH:MM format)".into(),
                            ),
                        });
                    }
                };
                let enabled = args
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.set_alarm(side, time, enabled).await
            }
            _ => unreachable!(),
        };

        match result {
            Ok(value) => Ok(ToolResult {
                success: true,
                output: serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_config::policy::SecurityPolicy;

    fn test_tool() -> EightSleepTool {
        let security = Arc::new(SecurityPolicy::default());
        EightSleepTool::new(
            "test@example.com".into(),
            "test-password".into(),
            Some("device-123".into()),
            30,
            security,
        )
    }

    #[test]
    fn tool_name_is_eight_sleep() {
        assert_eq!(test_tool().name(), "eight_sleep");
    }

    #[test]
    fn parameters_schema_has_required_action() {
        let schema = test_tool().parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("action")));
    }

    #[test]
    fn parameters_schema_defines_all_actions() {
        let schema = test_tool().parameters_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let action_strs: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
        assert!(action_strs.contains(&"get_bed_state"));
        assert!(action_strs.contains(&"get_metrics"));
        assert!(action_strs.contains(&"set_temperature"));
        assert!(action_strs.contains(&"set_priming"));
        assert!(action_strs.contains(&"set_alarm"));
    }

    #[test]
    fn parameters_schema_has_side_enum() {
        let schema = test_tool().parameters_schema();
        let sides = schema["properties"]["side"]["enum"].as_array().unwrap();
        let side_strs: Vec<&str> = sides.iter().filter_map(|v| v.as_str()).collect();
        assert!(side_strs.contains(&"left"));
        assert!(side_strs.contains(&"right"));
    }

    #[tokio::test]
    async fn execute_missing_action_returns_error() {
        let result = test_tool().execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("action"));
    }

    #[tokio::test]
    async fn execute_unknown_action_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "invalid"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Unknown action"));
    }

    #[tokio::test]
    async fn execute_set_temperature_missing_side_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "set_temperature", "temperature": 5}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("side"));
    }

    #[tokio::test]
    async fn execute_set_temperature_missing_temperature_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "set_temperature", "side": "left"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("temperature"));
    }

    #[tokio::test]
    async fn execute_set_priming_missing_side_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "set_priming", "enabled": true}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("side"));
    }

    #[tokio::test]
    async fn execute_set_alarm_missing_side_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "set_alarm", "time": "07:00"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("side"));
    }

    #[tokio::test]
    async fn execute_set_alarm_missing_time_returns_error() {
        let result = test_tool()
            .execute(json!({"action": "set_alarm", "side": "right"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("time"));
    }

    // -- Mock-server integration tests --

    fn mock_login_response() -> serde_json::Value {
        json!({
            "session": {
                "accessToken": "test-jwt-token",
                "expirationDate": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() + 3600,
                "userId": "user-1"
            }
        })
    }

    #[tokio::test]
    async fn mock_get_bed_state() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = server.uri();

        Mock::given(method("POST"))
            .and(path("/v1/users/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_login_response()))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/devices/device-123"))
            .and(header("Authorization", "Bearer test-jwt-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "device-123",
                "temperature": {"left": {"currentTarget": 5}, "right": {"currentTarget": -10}},
                "priming": {"left": {"enabled": true}, "right": {"enabled": false}}
            })))
            .mount(&server)
            .await;

        let security = Arc::new(SecurityPolicy::default());
        let tool = EightSleepTool::new(
            "test@example.com".into(),
            "test-password".into(),
            Some("device-123".into()),
            30,
            security,
        )
        .with_api_base(format!("{base}/v1"));

        let result = tool
            .execute(json!({"action": "get_bed_state"}))
            .await
            .unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
            result.error
        );
        let body: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(body["id"], "device-123");
    }

    #[tokio::test]
    async fn mock_set_temperature() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = server.uri();

        Mock::given(method("POST"))
            .and(path("/v1/users/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_login_response()))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/v1/devices/device-123"))
            .and(header("Authorization", "Bearer test-jwt-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
            .mount(&server)
            .await;

        let security = Arc::new(SecurityPolicy::default());
        let tool = EightSleepTool::new(
            "test@example.com".into(),
            "test-password".into(),
            Some("device-123".into()),
            30,
            security,
        )
        .with_api_base(format!("{base}/v1"));

        let result = tool
            .execute(json!({"action": "set_temperature", "side": "left", "temperature": 5}))
            .await
            .unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
            result.error
        );
    }

    #[tokio::test]
    async fn mock_login_failure() {
        use wiremock::matchers::method;
        use wiremock::matchers::path as path_match;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = server.uri();

        Mock::given(method("POST"))
            .and(path_match("/v1/users/login"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"error": "invalid credentials"})),
            )
            .mount(&server)
            .await;

        let security = Arc::new(SecurityPolicy::default());
        let tool = EightSleepTool::new(
            "bad@example.com".into(),
            "wrong-password".into(),
            None,
            30,
            security,
        )
        .with_api_base(format!("{base}/v1"));

        let result = tool
            .execute(json!({"action": "get_bed_state"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("login failed"));
    }

    #[tokio::test]
    async fn mock_auto_detect_device() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = server.uri();

        Mock::given(method("POST"))
            .and(path("/v1/users/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_login_response()))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/users/me"))
            .and(header("Authorization", "Bearer test-jwt-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "user": {
                    "devices": [{"id": "auto-detected-1", "name": "Pod Pro"}]
                }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/devices/auto-detected-1"))
            .and(header("Authorization", "Bearer test-jwt-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "auto-detected-1",
                "temperature": {"left": {"currentTarget": 0}, "right": {"currentTarget": 0}}
            })))
            .mount(&server)
            .await;

        let security = Arc::new(SecurityPolicy::default());
        let tool = EightSleepTool::new(
            "test@example.com".into(),
            "test-password".into(),
            None, // no device_id — auto-detect
            30,
            security,
        )
        .with_api_base(format!("{base}/v1"));

        let result = tool
            .execute(json!({"action": "get_bed_state"}))
            .await
            .unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
            result.error
        );
        let body: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(body["id"], "auto-detected-1");
    }

    #[test]
    fn cached_token_expiry_logic() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let valid = CachedToken {
            access_token: "tok".into(),
            expires_at: now + 120,
        };
        assert!(!valid.is_expired());

        let expired = CachedToken {
            access_token: "tok".into(),
            expires_at: now - 10,
        };
        assert!(expired.is_expired());

        // Within 60s buffer → considered expired
        let almost = CachedToken {
            access_token: "tok".into(),
            expires_at: now + 30,
        };
        assert!(almost.is_expired());
    }
}
