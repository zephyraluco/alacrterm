# Crate boundaries

The application is being refactored in independently verified stages. Configuration
formats, shell behavior and UI interactions remain compatible throughout.

| Crate | Responsibility |
| --- | --- |
| alacrterm | Application entry, windows, layout, UI adapters and event handling |
| settings | UI-independent configuration models, defaults and persistence |
| platform | Platform initialization and bundled WayX service |
| session | Saved session data and connection request policy |
| transfer | Transfer state, progress and cancellation |
| terminal_view | Terminal rendering, input and completion interaction |
| terminal | Emulator, PTY and shell integration |
| ssh | SSH, SFTP and authenticated X11 forwarding |
| util | Reusable tools without application or UI dependencies |

Dependencies point towards lower-level capabilities; libraries must not depend on
alacrterm. GPUI globals and conversion to rendering types belong to UI adapters.
Session identifiers, terminal processes and SSH protocol state retain their
existing owners rather than introducing competing lifecycle state machines.

## Validation baseline

Before extraction, workspace cargo check passes and all 49 application tests pass.
The terminal crate has previously documented hyperlink/path test failures; a full
workspace test pass is not claimed. Native Windows/macOS checks require their
platform toolchains and cannot be replaced by Linux checks.

Stages: settings, platform, session, transfer, utility/dependency cleanup. Each
stage receives targeted tests and workspace compilation before its own commit.

## Implemented ownership

- `settings` owns app/terminal file models, completion policy, configuration path
  selection, preference persistence and tolerant keymap parsing. It depends only
  on serde/serde_json and general utilities. The existing `config/*.json` formats
  and executable-directory → repository fallback are unchanged. Saving terminal
  preferences preserves palette and unknown fields.
- `session` owns saved records/folders, stable IDs, insert/edit/remove/move rules,
  serialization and connection requests. The sidebar owns selection, expanded
  rows and GPUI notifications. Live terminal/SSH state and reconnect remain in
  the existing backend and view; session does not introduce another state machine.
- `transfer` owns active transfer identities, progress and cancellation. Cancelled
  workers remain visible until completion. The application's transfer adapter
  receives worker events and forwards UI updates; SSH owns the actual SFTP worker.
- `platform` owns ConPTY initialization, WayX processes, readiness, shutdown and
  display generation endpoints. `ssh` owns forwarding cookies and X11 channel
  authentication. Application controls call platform directly.
- `util` owns a reusable file cache with shared leases, process locks and stale
  cleanup, byte formatting, JSON comment stripping and existing path/shell tools.
  Cache roots and GPUI global registration remain application policy. A cache root
  must be dedicated to the caller, because startup removes stale instance folders.

`alacrterm` retains settings/theme rendering adapters, file/preview UI workflows,
window state, focus, layout, dialogs and event dispatch. Rendering-specific types
such as colors, pixels and font weights remain in terminal_view/GPUI adapters.

## Verification after extraction

- Workspace all-target compilation and library Clippy performance checks pass.
- Settings, session, transfer, util, terminal_view and application tests pass;
  tests moved with the code, so application test counts decrease accordingly.
- Configuration preservation, stable session editing and drag paths, transfer
  cancellation and stale worker progress have focused coverage.
- SSH protocol roundtrips and X11 authentication tests pass. Actual bundled WayX
  restart/startup-cancellation and concurrent-session X11 tests also pass on Linux.
- Xvfb GUI regression covers local terminals, saved SSH sessions, local/remote
  project completion, middle-word insertion and password-input isolation.

Windows/macOS native execution still requires validation on their respective hosts.
