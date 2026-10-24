use crate::quota::{decide, is_stale, Credits, Decision, Window};
use crate::sessions::LocatedMeasurement;
use chrono::{DateTime, Local, Utc};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct WindowReport {
    used_percent: Option<f64>,
    measured_used_percent: Option<f64>,
    window_minutes: Option<u64>,
    window_length: Option<String>,
    resets_at: Option<String>,
    stale: bool,
}

#[derive(Serialize)]
pub struct Report {
    status: Decision,
    pub exit_code: u8,
    source: PathBuf,
    measured_at: String,
    timestamp_source: &'static str,
    age_seconds: u64,
    age: String,
    threshold: f64,
    primary: Option<WindowReport>,
    secondary: Option<WindowReport>,
    credits: Option<Credits>,
    rate_limit_reached_type: Option<Value>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn new(located: LocatedMeasurement, now: DateTime<Utc>, threshold: f64) -> Self {
        let measurement = located.measurement;
        let limits = measurement.rate_limits;
        let status = decide(&limits, now, threshold);
        let mut warnings = Vec::new();
        let primary = window_report("Primary", limits.primary.as_ref(), now, &mut warnings);
        let secondary = window_report("Secondary", limits.secondary.as_ref(), now, &mut warnings);
        if measurement.measured_at > now {
            warnings.push("Measurement timestamp is in the future; check the system clock.".into());
        }
        if status == Decision::NoData {
            warnings.push(
                "No quota data: the latest rate_limits event contains no usable measurement."
                    .into(),
            );
        }
        let age_seconds = now
            .signed_duration_since(measurement.measured_at)
            .num_seconds()
            .max(0) as u64;
        Self {
            exit_code: status.exit_code(),
            status,
            source: located.path,
            measured_at: local_time(measurement.measured_at),
            timestamp_source: measurement.timestamp_source,
            age_seconds,
            age: human_duration(u128::from(age_seconds)),
            threshold,
            primary,
            secondary,
            credits: limits.credits,
            rate_limit_reached_type: limits.rate_limit_reached_type,
            warnings,
        }
    }

    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        for (name, window) in [("Primary", &self.primary), ("Secondary", &self.secondary)] {
            let Some(window) = window else {
                lines.push(format!("{name}: unavailable"));
                continue;
            };
            let usage = window
                .used_percent
                .map(|used| format!("{used}% used"))
                .unwrap_or_else(|| "usage unknown".into());
            let duration = window.window_length.as_deref().unwrap_or("unknown window");
            let reset = window.resets_at.as_deref().unwrap_or("unknown");
            let stale = if window.stale {
                " [STALE: window has reset]"
            } else {
                ""
            };
            lines.push(format!(
                "{name}: {usage} / {duration}; resets {reset}{stale}"
            ));
        }
        let credits = self
            .credits
            .as_ref()
            .map(|credits| {
                let balance = credits.balance.as_deref().unwrap_or("unknown balance");
                if credits.unlimited == Some(true) {
                    return format!("unlimited (balance: {balance})");
                }
                match credits.has_credits {
                    Some(false) => format!("none (balance: {balance})"),
                    _ => balance.to_owned(),
                }
            })
            .unwrap_or_else(|| "unavailable".into());
        lines.push(format!("Credits: {credits}"));
        lines.push(format!(
            "Measurement: {} ago ({}, {}); {}",
            self.age,
            self.measured_at,
            self.timestamp_source,
            self.source.display()
        ));
        if let Some(reached) = &self.rate_limit_reached_type {
            lines.push(format!("Rate limit reached: {reached}"));
        }
        if self.status == Decision::Blocked {
            lines.push(format!("Quota blocked (threshold: {}%).", self.threshold));
        }
        lines.join("\n") + "\n"
    }
}

fn window_report(
    name: &str,
    window: Option<&Window>,
    now: DateTime<Utc>,
    warnings: &mut Vec<String>,
) -> Option<WindowReport> {
    let window = window?;
    let stale = is_stale(window, now);
    if stale {
        warnings.push(format!("{name} measurement is STALE: the window has reset; current usage is unknown, likely reset."));
    }
    Some(WindowReport {
        used_percent: if stale { None } else { window.used_percent },
        measured_used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        window_length: window
            .window_minutes
            .map(|minutes| human_duration(u128::from(minutes) * 60)),
        resets_at: window
            .resets_at
            .and_then(|reset| DateTime::from_timestamp(reset, 0))
            .map(local_time),
        stale,
    })
}

fn local_time(timestamp: DateTime<Utc>) -> String {
    timestamp.with_timezone(&Local).to_rfc3339()
}

fn human_duration(mut seconds: u128) -> String {
    let mut parts = Vec::new();
    for (unit, length) in [
        ("w", 604800),
        ("d", 86400),
        ("h", 3600),
        ("m", 60),
        ("s", 1),
    ] {
        let amount = seconds / length;
        seconds %= length;
        if amount != 0 {
            parts.push(format!("{amount}{unit}"));
        }
    }
    if parts.is_empty() {
        "0s".into()
    } else {
        parts.join(" ")
    }
}
