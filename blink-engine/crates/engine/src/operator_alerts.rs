//! Rate-limited operator alerts for live safety events.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tracing::warn;

static LAST_ALERT_MS: OnceLock<Mutex<HashMap<String, u128>>> = OnceLock::new();

pub fn emit_operator_alert(
    kind: &str,
    severity: &str,
    key: &str,
    message: &str,
    fields: Value,
) -> bool {
    if !env_bool("BLINK_OPERATOR_ALERTS_ENABLED", true) {
        return false;
    }

    let now_ms = current_time_ms();
    let rate_limit_ms = env_u128("BLINK_OPERATOR_ALERT_RATE_LIMIT_MS", 300_000);
    if rate_limit_ms > 0 && !reserve_rate_limit_slot(key, now_ms, rate_limit_ms) {
        return false;
    }

    let payload = json!({
        "timestamp_ms": now_ms,
        "kind": kind,
        "severity": severity,
        "key": key,
        "message": message,
        "fields": fields,
    });

    let path = std::env::var("BLINK_OPERATOR_ALERTS_PATH")
        .unwrap_or_else(|_| "logs/operator_alerts.jsonl".to_string());
    if let Err(err) = append_jsonl(&path, &payload) {
        warn!(path, err = %err, "operator alert append failed");
    }
    post_webhook_if_configured(payload);
    true
}

fn reserve_rate_limit_slot(key: &str, now_ms: u128, rate_limit_ms: u128) -> bool {
    let mut last_by_key = LAST_ALERT_MS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(last_ms) = last_by_key.get(key) {
        if now_ms.saturating_sub(*last_ms) < rate_limit_ms {
            return false;
        }
    }
    last_by_key.insert(key.to_string(), now_ms);
    true
}

fn append_jsonl(path: &str, payload: &Value) -> std::io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{payload}")?;
    Ok(())
}

fn post_webhook_if_configured(payload: Value) {
    let Some(url) = std::env::var("BLINK_OPERATOR_ALERT_WEBHOOK_URL")
        .ok()
        .or_else(|| std::env::var("ALERT_WEBHOOK_URL").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return;
    };

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        if let Err(err) = client.post(url).json(&payload).send().await {
            warn!(err = %err, "operator alert webhook failed");
        }
    });
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u128(name: &str, default: u128) -> u128 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u128>().ok())
        .unwrap_or(default)
}

fn current_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn alert_jsonl_is_rate_limited_by_key() {
        let _guard = env_lock();
        let path = std::env::temp_dir().join(format!(
            "blink-operator-alerts-test-{}-{}.jsonl",
            std::process::id(),
            current_time_ms()
        ));
        let _ = std::fs::remove_file(&path);

        std::env::set_var("BLINK_OPERATOR_ALERTS_ENABLED", "true");
        std::env::set_var("BLINK_OPERATOR_ALERTS_PATH", &path);
        std::env::set_var("BLINK_OPERATOR_ALERT_RATE_LIMIT_MS", "600000");
        std::env::remove_var("BLINK_OPERATOR_ALERT_WEBHOOK_URL");
        std::env::remove_var("ALERT_WEBHOOK_URL");

        assert!(emit_operator_alert(
            "test",
            "warn",
            "test-key",
            "first",
            json!({"n": 1})
        ));
        assert!(!emit_operator_alert(
            "test",
            "warn",
            "test-key",
            "second",
            json!({"n": 2})
        ));
        assert!(emit_operator_alert(
            "test",
            "warn",
            "other-key",
            "third",
            json!({"n": 3})
        ));

        let body = std::fs::read_to_string(&path).expect("alert file readable");
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"message\":\"first\""));
        assert!(lines[1].contains("\"message\":\"third\""));

        let _ = std::fs::remove_file(&path);
        std::env::remove_var("BLINK_OPERATOR_ALERTS_ENABLED");
        std::env::remove_var("BLINK_OPERATOR_ALERTS_PATH");
        std::env::remove_var("BLINK_OPERATOR_ALERT_RATE_LIMIT_MS");
    }
}
