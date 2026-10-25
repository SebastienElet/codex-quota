use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
pub struct Window {
    pub used_percent: Option<f64>,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Credits {
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RateLimits {
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
    pub credits: Option<Credits>,
    pub rate_limit_reached_type: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct Measurement {
    pub rate_limits: RateLimits,
    pub measured_at: DateTime<Utc>,
    pub timestamp_source: &'static str,
}

pub fn parse_event(line: &str, mtime: DateTime<Utc>) -> Result<Option<Measurement>, String> {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return Ok(None);
    };
    if event["type"] != "event_msg" || event["payload"]["type"] != "token_count" {
        return Ok(None);
    }
    let limits = &event["payload"]["rate_limits"];
    if limits.is_null() {
        return Ok(None);
    }
    let rate_limits: RateLimits = serde_json::from_value(limits.clone())
        .map_err(|error| format!("invalid rate_limits: {error}"))?;
    for window in [&rate_limits.primary, &rate_limits.secondary]
        .into_iter()
        .flatten()
    {
        if window
            .used_percent
            .is_some_and(|used| !used.is_finite() || used < 0.0)
        {
            return Err("invalid used_percent: expected a non-negative number".into());
        }
        if window
            .resets_at
            .is_some_and(|reset| DateTime::from_timestamp(reset, 0).is_none())
        {
            return Err("invalid resets_at: timestamp is out of range".into());
        }
    }
    let timestamp = event["timestamp"]
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok());
    Ok(Some(Measurement {
        rate_limits,
        measured_at: timestamp
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or(mtime),
        timestamp_source: if timestamp.is_some() {
            "event"
        } else {
            "file_mtime"
        },
    }))
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Ok,
    Blocked,
    NoData,
}

impl Decision {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Blocked => 1,
            Self::NoData => 2,
        }
    }
}

pub fn is_stale(window: &Window, now: DateTime<Utc>) -> bool {
    window
        .resets_at
        .is_some_and(|reset| reset <= now.timestamp())
}

pub fn decide(limits: &RateLimits, now: DateTime<Utc>, threshold: f64) -> Decision {
    let windows: Vec<_> = [&limits.primary, &limits.secondary]
        .into_iter()
        .flatten()
        .collect();
    if windows.iter().any(|window| {
        !is_stale(window, now) && window.used_percent.is_some_and(|used| used >= threshold)
    }) {
        return Decision::Blocked;
    }
    let all_stale = !windows.is_empty() && windows.iter().all(|window| is_stale(window, now));
    if limits.rate_limit_reached_type.is_some() && !all_stale {
        return Decision::Blocked;
    }
    let has_reading = windows.iter().any(|window| window.used_percent.is_some());
    let has_credits = limits.credits.as_ref().is_some_and(|credits| {
        credits.balance.is_some() || credits.has_credits.is_some() || credits.unlimited.is_some()
    });
    if !all_stale && !has_reading && !has_credits {
        return Decision::NoData;
    }
    Decision::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_observed_token_count_with_outer_timestamp_and_unknown_fields() {
        let mtime = DateTime::from_timestamp(0, 0).unwrap();
        let measurement = parse_event(include_str!("../tests/fixtures/token-count.jsonl"), mtime)
            .unwrap()
            .expect("quota from the real event shape");
        assert_eq!(
            measurement.measured_at.to_rfc3339(),
            "2026-10-24T09:06:39.567+00:00"
        );
        let window = measurement.rate_limits.primary.unwrap();
        assert_eq!(window.used_percent, Some(83.0));
        assert_eq!(window.window_minutes, Some(10080));
        assert_eq!(window.resets_at, Some(1793388805));
        assert!(measurement.rate_limits.secondary.is_none());
        let credits = measurement.rate_limits.credits.unwrap();
        assert_eq!(credits.balance.as_deref(), Some("62500.0000000000"));
        assert_eq!(credits.has_credits, Some(true));
        assert_eq!(credits.unlimited, Some(false));
        assert!(measurement.rate_limits.rate_limit_reached_type.is_none());
    }

    fn limits(json: &str) -> RateLimits {
        serde_json::from_str(json).unwrap()
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1000, 0).unwrap()
    }

    #[test]
    fn blocks_at_threshold_in_either_current_window() {
        for name in ["primary", "secondary"] {
            let limits = limits(&format!(
                r#"{{"{name}":{{"used_percent":99,"resets_at":2000}}}}"#
            ));
            assert_eq!(decide(&limits, now(), 99.0), Decision::Blocked);
            assert_eq!(decide(&limits, now(), 100.0), Decision::Ok);
        }
    }

    #[test]
    fn explicit_reached_type_blocks_unless_all_windows_have_reset() {
        for reached in [r#""primary""#, "{}", "false", "0"] {
            let current = limits(&format!(r#"{{"rate_limit_reached_type":{reached}}}"#));
            assert_eq!(decide(&current, now(), 99.0), Decision::Blocked);
            let expired = limits(&format!(
                r#"{{"primary":{{"used_percent":100,"resets_at":1000}},"rate_limit_reached_type":{reached}}}"#
            ));
            assert_eq!(decide(&expired, now(), 99.0), Decision::Ok);
        }
    }

    #[test]
    fn missing_or_null_measurements_are_no_data() {
        for json in [
            "{}",
            r#"{"primary":null,"secondary":null,"credits":null}"#,
            r#"{"primary":{},"credits":{}}"#,
        ] {
            assert_eq!(decide(&limits(json), now(), 99.0), Decision::NoData);
        }
    }

    #[test]
    fn stale_window_does_not_hide_another_current_blocked_window() {
        let limits = limits(
            r#"{"primary":{"used_percent":100,"resets_at":999},"secondary":{"used_percent":99,"resets_at":2000}}"#,
        );
        assert_eq!(decide(&limits, now(), 99.0), Decision::Blocked);
        assert!(is_stale(limits.primary.as_ref().unwrap(), now()));
        assert!(!is_stale(limits.secondary.as_ref().unwrap(), now()));
    }

    #[test]
    fn reset_at_now_is_stale_and_missing_reset_still_checks_usage() {
        let expired = limits(r#"{"primary":{"used_percent":100,"resets_at":1000}}"#);
        assert_eq!(decide(&expired, now(), 99.0), Decision::Ok);
        let unknown = limits(r#"{"primary":{"used_percent":100}}"#);
        assert_eq!(decide(&unknown, now(), 99.0), Decision::Blocked);
    }

    #[test]
    fn credits_only_can_be_reported_without_windows() {
        let limits = limits(r#"{"credits":{"unlimited":true,"balance":null}}"#);
        assert_eq!(decide(&limits, now(), 99.0), Decision::Ok);
    }

    #[test]
    fn timestamp_missing_invalid_or_null_falls_back_to_mtime() {
        for timestamp in ["null", r#""bad-date""#, "123"] {
            let line = format!(
                r#"{{"timestamp":{timestamp},"type":"event_msg","payload":{{"type":"token_count","rate_limits":{{}}}}}}"#
            );
            let measurement = parse_event(&line, now()).unwrap().unwrap();
            assert_eq!(measurement.measured_at, now());
            assert_eq!(measurement.timestamp_source, "file_mtime");
        }
        let measurement = parse_event(
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{}}}"#,
            now(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(measurement.measured_at, now());
    }

    #[test]
    fn skips_unrelated_null_and_truncated_events() {
        for line in [
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"other","rate_limits":{}}}"#,
            r#"{"type":"response_item","payload":{"type":"token_count","rate_limits":{}}}"#,
            r#"{"type":"event_msg""#,
            "null",
        ] {
            assert!(parse_event(line, now()).unwrap().is_none());
        }
    }

    #[test]
    fn rejects_invalid_known_fields_instead_of_reporting_available_quota() {
        for invalid in [
            r#"{"primary":{"used_percent":"99"}}"#,
            r#"{"primary":{"used_percent":-1}}"#,
            r#"{"secondary":{"resets_at":9223372036854775807}}"#,
            r#"{"primary":{"window_minutes":-1}}"#,
            r#"{"credits":{"balance":42}}"#,
            "[]",
        ] {
            let line = format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","rate_limits":{invalid}}}}}"#
            );
            assert!(parse_event(&line, now()).is_err(), "{invalid}");
        }
    }
}
