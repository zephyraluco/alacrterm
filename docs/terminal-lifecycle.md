# Terminal and transfer ownership

| Owner | Resources | End of lifetime |
| --- | --- | --- |
| Terminal instance | Backend, SSH output pump, emulator and sync timeout | Shutdown backend and cancel retained tasks |
| TerminalView | Startup, blink and input-recovery tasks; focus/model subscriptions | Cancel tasks and release subscriptions |
| TransferManager | Active transfer receivers, cancellation tokens and completion metadata | Cancel remaining work when the application service is released |
| AppRoot | Transfer completion subscription | Dispatch preview/system-open effects and optional file-list refresh |

The terminal model owns Connecting/Running/Exited state. Views derive exit state
from that model; local exit no longer remains connected. Reconnect is allowed
only for an exited SSH terminal, retains its grid/history, advances a connection
epoch and replaces its output task. Old-epoch deliveries cannot modify the new
connection. Repeated close events do not dispatch duplicate terminal-close events.

SshSession cancellation covers connection setup and host-key confirmation as well
as normal I/O. Dropping a session cancels its pending work and closes its private
runtime; it does not require the UI to respond to an obsolete host-key dialog.

TerminalView retains its startup/recovery tasks and terminal observer instead of
letting them outlive their view. The short delayed local child kill remains an
intentional terminal-owned teardown action after shutdown.

Transfers belong to the application service rather than a sidebar. Active receiver
tasks are retained by TransferManager. At completion their handles are released
without cancelling the final UI delivery. The manager emits one completion event;
AppRoot handles preview/system-open effects and file-list refresh. Transfer data
retains its SFTP handle and cache lease independently of the terminal's SSH shell.
Closing a terminal therefore does not cancel its transfers. Existing application
close/cancel/wait controls remain in effect; dropping the transfer owner cancels
all remaining tokens.

Validation covers lifecycle epochs and duplicate exits, cancellation during an
unresponsive handshake and host-key confirmation, file transfer after its terminal
connection closes, transfer-owner drop cancellation, and GUI reconnect/preview.
