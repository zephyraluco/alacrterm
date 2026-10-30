# CI and installers

- `test`: every branch push and pull request. Rustfmt runs before native dependency installation. Clippy, Python and C state-machine regressions gate parallel Linux, Windows and macOS application/library tests and SSH local-server tests. macOS tests compile the Sparkle bridge.
- `package`: every branch push and pull request; Linux x64 deb (Ubuntu 24.04 or compatible), Windows x64 per-user Inno Setup installer, and macOS ARM64 dmg. Installer artifacts, SHA-256 files and per-platform build manifests are retained for seven days.
- `release`: every tag push; reuses successful branch-push Test and Package runs at the exact commit SHA. Downloads are bound to the selected run ID, never a PR merge build. Expired artifacts trigger a complete three-platform rebuild; normal releases do not rebuild. Sparkle ZIP signing happens separately, before signed feeds are generated. Tags do not trigger the standalone test workflow.

The package version comes from `crates/alacrterm/Cargo.toml`; update it before tagging (recommended tag: `v<version>`). Build manually with `cargo build --release --locked -p alacrterm`, then `python scripts/packaging/package.py` on the matching OS/architecture. Python 3.11+ is required. Windows needs Inno Setup 6; macOS needs Xcode command-line tools; Linux needs dpkg-deb. Global themes and terminal palettes are installed beside the executable under `themes/global` and `themes/terminal`; bundled WayX runtime files and Windows ConPTY files from `plugins/windows/conpty` are installed under `plugins/conpty`. Packaged WayX directories are installed directly under `plugins/wayx-<platform>-<architecture>`, without the source tree's platform directory.

The application icon comes from `assets/app-icon.ico`. Windows embeds it in the executable and uses it for the installer; Linux installs its original PNG frames in the hicolor icon theme; macOS packages those frames as `Contents/Resources/app-icon.icns`. Keep the 128px and 256px PNG frames when replacing the ICO.

All macOS packages use ad-hoc signing without Developer ID signing or notarization; tag releases still require Sparkle upgrade signatures. macOS may block the first launch; after attempting to open the app, allow it in System Settings → Privacy & Security. Windows installers are not Authenticode signed. Release publishing waits for checks, tests and all three packages to succeed. See [online update setup](../../docs/architecture.md#在线升级) for required public keys, secrets and native acceptance checks.

Release-build user configuration (`app.json`, `terminal.json`, `sessions.json`, optional `keymap.json`) is stored exclusively in the system user configuration directory:

- Windows: `%APPDATA%\Alacrterm` (Roaming).
- Linux: `$XDG_CONFIG_HOME/alacrterm`, default `~/.config/alacrterm`.
- macOS: `~/Library/Application Support/Alacrterm`.

Debug/development builds instead read and write the repository-root `config/` directory, which is git-ignored and not packaged. There is no fallback between development and release configuration. Configuration directories are created on first save. Installation/source directories are not read for user configuration, and old files are not migrated. Bundled themes stay beside the executable.

CI workflows call the standalone scripts in this directory for Sparkle preparation, build identity/integrity manifests, CI resolution, artifact download/validation, signing, feed generation and publication. Python regression tests live in `scripts/test`; run them with `python -m unittest discover -s scripts/test -p 'test_*.py'`. The native Sparkle state regression test is available through `bash scripts/test/test_sparkle_state.sh`.

## Release identity and recovery

`artifacts.py` records commit, version, run ID/attempt, public-key fingerprints, and the exact files with sizes/SHA-256 in `build-<platform>.json`. Downloads must match the tag SHA, selected successful source run, current public keys and complete three-platform file set. Failed-job reruns may contribute different attempts within the same run. A key change requires rebuilding the installers; it cannot be repaired by signing old binaries with a new key.

`resolve_ci.py` waits up to an hour for branch CI. Missing runs fail after five minutes with a push-to-branch instruction. Failed/cancelled runs fail immediately. Expired artifacts are rebuilt; recent missing/duplicate platforms fail rather than mixing sources. Rebuild artifacts belong to the current Release run and are consumed only after its reusable Package job succeeds.

`publish_release.py` creates or resumes a draft. Uploads may replace draft attachments, but unexpected attachments require review. All uploaded bytes are downloaded and verified before publication. `release-manifest.json` identifies the commit/tag and hashes every public attachment. Already published versions are verified and left unchanged; an incomplete or changed version fails instead of being overwritten. Re-generated feed timestamps do not force an immutable version to be re-uploaded.

Publication is serialized with `cancel-in-progress: false`. GitHub concurrency is not a FIFO queue and may replace pending runs; rerun a cancelled pending release if needed. Numeric stable-version comparison prevents older releases from replacing latest. Manual publication outside this workflow is not covered by its concurrency lock.

Checks/test/package jobs have 30/45/60 minute limits; CI resolution has a 65 minute job limit, and signing/publication have 30 minute limits. PR runs cancel obsolete PR runs; branch runs are retained because tags may depend on their results. Rust caches are separated by purpose. The pinned Sparkle archive is cached and revalidated by SHA-256 before extraction. CI uses Python 3.12; local scripts require 3.11+.

None of these checks performs an actual Windows installation, macOS application replacement or privileged Linux package installation. Those still require native acceptance tests.
