# Architecture implementation progress

Current implementation follows the incremental architecture review. Each stage is
verified and committed separately; this document distinguishes delivered changes
from the remaining design.

Delivered configuration reliability:
- Explicit-path SettingsStore reads and merges JSON documents.
- Malformed JSON and non-object configurations are never silently overwritten.
- Settings and session writes use same-directory temporary files, flush and atomic
  replacement. Existing permissions are retained; save failures propagate.
- Theme preference updates preserve unknown fields.
- Terminal numeric overrides are validated centrally using the UI's bounds.
- Invalid external numeric overrides fall back to renderer defaults; invalid save
  requests fail. A failed terminal preference save does not apply new render values.

Remaining architecture stages:
- Separate resource/config/cache/state paths with explicit portable-mode migration;
  move persisted credentials behind AuthRef and a credential store.
- Extract document/save models and unify preview uploads with transfer ownership.
- Establish workspace layout authority and inject terminal models into views.
- Extract directory request ownership and remove terminal dependencies from saved
  session data.
- Introduce typed terminal capabilities and bounded input/latest-size scheduling.
- Measure load before changing executor sharing or parser threading.

Validation for the delivered changes: settings/session/util/application unit tests,
util doctests and workspace all-target compilation. Native filesystem replacement
behavior on Windows/macOS still needs native platform validation.
