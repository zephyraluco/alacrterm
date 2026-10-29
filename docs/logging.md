# Runtime logging

Existing application `log::error!`, `warn!`, `info!`, `debug!` and `trace!` calls are retained. `tracing-log` forwards them to the tracing subscriber for filtering and file output. Dependencies using either facade share that output. No business log sites, messages, levels, spans, panic hooks or logging settings pages are added or migrated. Direct stderr output (`eprintln!`) remains unchanged and is not captured by the bridge.

## Files

| Platform | Directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Alacrterm\logs` |
| macOS | `~/Library/Logs/Alacrterm` |
| Linux | `$XDG_STATE_HOME/alacrterm/logs`, default `~/.local/state/alacrterm/logs` |

These locations apply to both development and installed builds. Files distinguish application/helper, build profile, PID, and a per-start identifier, for example `alacrterm-release-12345-1793250000000000000.2026-10-29.log`. Each process writes independently and rotates daily in UTC. The current UTC date and preceding six dates are retained: seven calendar dates in total. There are no per-file or total size limits. Cleanup runs at startup and hourly, and removes only recognized application/helper log files, not symlinks, installer logs or unrelated files.

A bounded background queue keeps file writes off the UI thread. Queue saturation can drop events. Normal shutdown drains the queue; the application also flushes during GPUI shutdown for native macOS termination, and the Windows helper returns an exit code rather than bypassing Rust destructors. Abrupt termination cannot guarantee final-event persistence. If a log directory cannot be created/opened, diagnostics fall back to stderr without preventing startup.

## Filtering

Files use readable timestamped text, with level and module target and without ANSI colors. Application modules default to `INFO` and dependency modules to `WARN`. Override with `RUST_LOG` before launch:

```sh
RUST_LOG=alacrterm=debug,ssh=debug alacrterm
```

A global value such as `RUST_LOG=debug` enables dependency diagnostics too. Empty or invalid filters use the defaults.

## Existing diagnostic content

Existing log sites, messages and levels are preserved. Existing DEBUG diagnostics can include terminal input and remote commands, so enabling those modules at DEBUG also writes that content to the log files. Other messages can include host addresses, paths and error details. Inspect logs before sharing them. The default INFO/WARN filters exclude the terminal-input and remote-command DEBUG sites. The logging infrastructure does not read private signing keys or configuration documents.

The update helper uses its own runtime log files in the same platform log directory. Its existing per-update `helper-error.log`, `result.json` and installer diagnostics remain in the update task directory and follow their existing lifecycle.

## Verification

```sh
cargo test --locked -p util --features logging
cargo clippy --locked --all-targets --all-features -- -D warnings
```

Logging tests cover seven-date cleanup boundaries, unrelated-file preservation, independent writers, queue flushing, filter fallback, unavailable directories, and real subprocess initialization including compatibility with legacy `log` events.
