# codex-quota

Read Codex quota snapshots from local session logs before starting work. Reports
primary/secondary usage, window lengths, local reset times, credits and measurement age.

```sh
cargo install --git https://github.com/SebastienElet/codex-quota

codex-quota
codex-quota && codex ...
codex-quota --threshold 95 --json
codex-quota --sessions-dir /path/to/sessions
```

Uses `$CODEX_HOME/sessions`, or `~/.codex/sessions`. Checks `rollout-*.jsonl`
files up to three directory levels deep, newest file modification time first,
and takes the last `event_msg` / `token_count` event with non-null `rate_limits`.
Falls back to older files when a file has no such event. Ignores malformed JSON
lines (including an incomplete trailing write); invalid known quota fields are errors.
Measurement age uses the event timestamp, falling back to file modification time.

The default threshold is **99%** (inclusive; configurable from 0 to 100).
Expired windows are marked **STALE**, warn, and report current usage as unknown.
They do not block; another unexpired window can still block. A non-null
`rate_limit_reached_type` blocks unless all reported windows have reset.
This is a local snapshot, not a live quota request. JSON includes the same decision,
exit code and warnings; stale percentages are preserved as `measured_used_percent`.

| Exit | Meaning |
| --- | --- |
| 0 | Below threshold, credits-only data, or expired windows |
| 1 | A current window meets the threshold, or a rate limit was reached |
| 2 | No session data or no usable quota measurement |
| 3 | Invalid arguments, invalid quota fields, or an I/O error |
