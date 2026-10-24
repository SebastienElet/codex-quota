use crate::quota::{parse_event, Measurement};
use chrono::{DateTime, Utc};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct LocatedMeasurement {
    pub measurement: Measurement,
    pub path: PathBuf,
}

pub fn latest_measurement(root: &Path) -> Result<Option<LocatedMeasurement>, String> {
    let mut files = Vec::new();
    match collect_files(root, 0, &mut files) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && !root.exists() => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot list sessions in {}: {error}",
                root.display()
            ))
        }
        Ok(()) => {}
    }
    files.sort_unstable_by(|left, right| right.cmp(left));
    for (mtime, path) in files {
        if let Some(measurement) = read_measurement(&path, DateTime::<Utc>::from(mtime))? {
            return Ok(Some(LocatedMeasurement { measurement, path }));
        }
    }
    Ok(None)
}

fn collect_files(
    directory: &Path,
    depth: usize,
    files: &mut Vec<(SystemTime, PathBuf)>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() && depth < 3 {
            collect_files(&entry.path(), depth + 1, files)?;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
            files.push((entry.metadata()?.modified()?, entry.path()));
        }
    }
    Ok(())
}

fn read_measurement(path: &Path, mtime: DateTime<Utc>) -> Result<Option<Measurement>, String> {
    let file =
        File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut latest = Ok(None);
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        match parse_event(&line, mtime) {
            Ok(None) => {}
            result => {
                latest =
                    result.map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))
            }
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SessionsDirectory;
    use std::fs::{File, FileTimes};
    use std::time::{Duration, UNIX_EPOCH};

    const OBSERVED: &str = include_str!("../tests/fixtures/token-count.jsonl");

    fn set_mtime(path: &Path, seconds: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }

    #[test]
    fn selects_newest_mtime_then_last_event_and_falls_back_past_empty_file() {
        let directory = SessionsDirectory::new();
        let older = directory.write("2026/10/23/rollout-z.jsonl", OBSERVED);
        set_mtime(&older, 100);
        let current = directory.write(
            "2026/10/24/rollout-a.jsonl",
            &format!(
                "{OBSERVED}{}\n{{\"unfinished\"",
                OBSERVED.replace("83.0", "95.0")
            ),
        );
        set_mtime(&current, 200);
        let empty = directory.write(
            "2026/10/24/rollout-new.jsonl",
            "{\"type\":\"session_meta\"}\n",
        );
        set_mtime(&empty, 300);
        directory.write("2026/10/24/unrelated.jsonl", OBSERVED);
        let selected = latest_measurement(directory.path()).unwrap().unwrap();
        assert_eq!(selected.path, current);
        assert_eq!(
            selected
                .measurement
                .rate_limits
                .primary
                .unwrap()
                .used_percent,
            Some(95.0)
        );
    }

    #[test]
    fn absent_empty_and_quota_free_directories_have_no_data() {
        let directory = SessionsDirectory::new();
        assert!(latest_measurement(&directory.path().join("missing"))
            .unwrap()
            .is_none());
        assert!(latest_measurement(directory.path()).unwrap().is_none());
        directory.write("2026/10/24/rollout-empty.jsonl", "{}\n");
        assert!(latest_measurement(directory.path()).unwrap().is_none());
    }

    #[test]
    fn invalid_latest_quota_is_an_error_and_does_not_resurrect_earlier_usage() {
        let directory = SessionsDirectory::new();
        directory.write(
            "2026/10/24/rollout-bad.jsonl",
            &format!("{OBSERVED}{}", OBSERVED.replace("83.0", "-1")),
        );
        let error = latest_measurement(directory.path()).err().unwrap();
        assert!(error.contains("invalid used_percent"));
        assert!(error.contains(":2:"));
    }

    #[test]
    fn missing_event_timestamp_uses_selected_file_mtime() {
        let directory = SessionsDirectory::new();
        let path = directory.write("2026/10/24/rollout-no-time.jsonl", r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":42}}}}"#);
        set_mtime(&path, 1234);
        let selected = latest_measurement(directory.path()).unwrap().unwrap();
        assert_eq!(selected.measurement.measured_at.timestamp(), 1234);
        assert_eq!(selected.measurement.timestamp_source, "file_mtime");
    }

    #[test]
    fn filesystem_errors_are_reported() {
        let directory = SessionsDirectory::new();
        let file = directory.write("file", "");
        assert!(latest_measurement(&file)
            .err()
            .unwrap()
            .contains("cannot list sessions"));
    }
}
