mod output;
mod quota;
mod sessions;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

use chrono::Utc;
use serde_json::json;
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "codex-quota — check quota from local Codex session logs

Usage: codex-quota [--sessions-dir <path>] [--threshold <pct>] [--json]

  --sessions-dir <path>  Override $CODEX_HOME/sessions or ~/.codex/sessions
  --threshold <pct>      Block at or above this percentage (0–100; default: 99)
  --json                Print a machine-readable report
  -h, --help            Print help
  -V, --version         Print version

Exit codes: 0 OK or stale; 1 quota blocked; 2 no data; 3 input or I/O error.
";

struct Options {
    sessions_dir: PathBuf,
    threshold: f64,
}

enum Invocation {
    Run(Options),
    Help,
    Version,
}

fn parse_arguments(arguments: Vec<OsString>) -> Result<Invocation, String> {
    let mut arguments = arguments.into_iter();
    let mut sessions_dir = None;
    let mut threshold = 99.0;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--sessions-dir") => {
                let path = arguments
                    .next()
                    .filter(|path| !path.is_empty())
                    .ok_or("--sessions-dir requires a path")?;
                sessions_dir = Some(PathBuf::from(path));
            }
            Some("--threshold") => {
                let value = arguments
                    .next()
                    .ok_or("--threshold requires a percentage")?;
                threshold = value
                    .to_str()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
                    .ok_or("--threshold must be a finite percentage between 0 and 100")?;
            }
            Some("--json") => {}
            Some("-h" | "--help") => return Ok(Invocation::Help),
            Some("-V" | "--version") => return Ok(Invocation::Version),
            _ => {
                return Err(format!(
                    "unknown argument: {} (use --help)",
                    argument.to_string_lossy()
                ))
            }
        }
    }
    let sessions_dir = match sessions_dir {
        Some(path) => path,
        None => default_sessions_dir()?,
    };
    Ok(Invocation::Run(Options {
        sessions_dir,
        threshold,
    }))
}

fn default_sessions_dir() -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("CODEX_HOME") {
        if home.is_empty() {
            return Err("CODEX_HOME is empty; set it or use --sessions-dir".into());
        }
        return Ok(PathBuf::from(home).join("sessions"));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .ok_or("cannot determine home directory; use --sessions-dir")?;
    Ok(PathBuf::from(home).join(".codex/sessions"))
}

fn write_stdout(text: &str) -> Result<(), String> {
    io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|error| format!("cannot write output: {error}"))
}

fn run(arguments: Vec<OsString>, json_output: bool) -> Result<u8, String> {
    let options = match parse_arguments(arguments)? {
        Invocation::Run(options) => options,
        Invocation::Help => {
            write_stdout(HELP)?;
            return Ok(0);
        }
        Invocation::Version => {
            write_stdout(concat!("codex-quota ", env!("CARGO_PKG_VERSION"), "\n"))?;
            return Ok(0);
        }
    };
    let Some(located) = sessions::latest_measurement(&options.sessions_dir)? else {
        let message = format!(
            "No quota data found in {}. Run Codex to record a token_count rate_limits event.",
            options.sessions_dir.display()
        );
        if json_output {
            write_stdout(&format!(
                "{}\n",
                json!({"status":"no_data", "exit_code":2, "message":message})
            ))?;
        } else {
            eprintln!("{message}");
        }
        return Ok(2);
    };
    let report = output::Report::new(located, Utc::now(), options.threshold);
    if json_output {
        let serialized = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot serialize report: {error}"))?;
        write_stdout(&(serialized + "\n"))?;
    } else {
        write_stdout(&report.text())?;
        for warning in &report.warnings {
            eprintln!("Warning: {warning}");
        }
    }
    Ok(report.exit_code)
}

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let json_output = arguments.iter().any(|argument| argument == "--json");
    match run(arguments, json_output) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if json_output {
                let message = json!({"status":"error", "exit_code":3, "message":error});
                if write_stdout(&format!("{message}\n")).is_err() {
                    eprintln!("codex-quota: {error}");
                }
            } else {
                eprintln!("codex-quota: {error}");
            }
            ExitCode::from(3)
        }
    }
}
