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
