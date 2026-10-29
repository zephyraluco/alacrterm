# Online updates

The application has one update model shared by the settings window and normal-mode title bar. Open Settings → 更新 to check, download or install an update. When an update is available, a 可升级 entry appears to the left of the X11/settings buttons in the normal-mode title bar. It opens a confirmation dialog showing the current and available versions, with 取消 and 立刻更新 buttons. Windows/Linux proceed from download and verification to installation preparation; cancelling the dialog stops its own download and prevents subsequent installation. macOS continues in Sparkle’s native update UI. Versions below the supported upgrade minimum open the official download page instead. Zoom mode does not display this entry. Checks run after 30 seconds and then every six hours on Windows/Linux when enabled; downloads are explicit. Sparkle owns scheduling and download/install UI on macOS. Debug builds never check online and use the ignored repository `config/update.json`; release settings use the platform user configuration directory. Automatic checking defaults to enabled, and is saved independently from other settings.

## Platform behavior

- **Windows x64**: signed JSON feed → verified exe in the user cache → background verification and helper preparation → helper ready acknowledgement → explicit restart confirmation → application and WayX shutdown → cached Rust helper waits for all installed app/WayX instances → Inno Setup runs silently in the original installation directory → application relaunches. The helper independently verifies the original signed envelope and package. Each attempt uses a unique task directory under the user's local data directory (`Alacrterm/updates/jobs/install-*/`). The helper acquires an OS file lock scoped to the installation and acknowledges readiness before the application can exit. Canceling confirmation terminates the waiting helper; a dead or timed-out helper cannot trigger application exit. Installation status is recorded in that task's `result.json`, checked against the running version on next start, then retained as `last-result.json`. The installer's log is retained as `installer.log`. Installation failure is reported; automatic rollback is not claimed.
- **macOS ARM64**: Sparkle 2.10.1, with its standard update UI and installer. The Rust bridge runs on the main thread. A relaunch requested by Sparkle is postponed for the application's confirmation; active transfers or dirty previews block this confirmation. The user can continue from Settings after finishing those tasks. The application exit coordinator also intercepts AppKit termination before GPUI teardown, covering normal quit and Sparkle paths that do not invoke the postpone callback. Confirmation rechecks connected terminal identities, transfers and unsaved previews immediately before proceeding. Sparkle dismissal, skip, cancellation and cycle completion clear transient update states; a deferred downloaded update remains distinct from an active download. The pinned framework archive is checked against its official SHA-256 before extraction. Normal package builds use ad-hoc signing; tag releases require Developer ID signing and notarization. Downloads use Sparkle's EdDSA signatures, independent from Apple code signing.
- **Linux x64 deb**: signed JSON feed and package integrity checks; installation opens `gdebi-gtk`, Discover or GNOME Software if found. Otherwise a safely quoted `sudo apt install /absolute/path.deb` command is shown. The application never asks for the sudo password and never overwrites `/usr/lib` itself. The system installation tool owns authorization, dependency resolution and installation results. Restart manually after successful installation.

Updating installs the entire release, including `themes`, WayX in `plugins`, Windows ConPTY, and updater components. User configuration directories are not part of the package and are not overwritten. Restart closes terminal/X11 sessions; local terminal processes are not restored. Active transfers and dirty previews must be resolved before initiating installation from Settings.

Installer verification and helper copying run on background threads; the UI remains responsive and prevents duplicate preparation while confirmation is open. Windows/Linux downloads use cancellable async network requests with a 30-second connection/idle timeout and 30-minute overall deadline; cancellation interrupts stalled headers or body reads and preserves incomplete downloads for resumption. Package hashing also checks for cancellation. Update states carry their signed release and package data directly.

Caches: `dirs::cache_dir()/Alacrterm/updates`. Results/logs: `dirs::data_local_dir()/Alacrterm/updates`. Configuration: `%APPDATA%/Alacrterm`, `$XDG_CONFIG_HOME/alacrterm` (default `~/.config/alacrterm`), or `~/Library/Application Support/Alacrterm`. A build without a configured public key refuses online updates and links to the manual download page.

## GitHub configuration

Repository **variables** (public):

| Variable | Value |
| --- | --- |
| `ALACRTERM_UPDATE_PUBLIC_KEYS` | One or more minisign public key base64 lines, separated by whitespace; do not include comment lines |
| `SPARKLE_PUBLIC_KEY` | Sparkle EdDSA public key, copied into `SUPublicEDKey` |

Repository **secrets**:

| Secret | Value |
| --- | --- |
| `MINISIGN_SECRET_KEY` | Complete minisign private key file |
| `MINISIGN_PASSWORD` | Password for the minisign key (may be empty for an unencrypted key) |
| `SPARKLE_PRIVATE_KEY` | Key exported by Sparkle `generate_keys`; base64 text |
| `APPLE_CERTIFICATE_P12` | Base64 Developer ID Application certificate and private key export |
| `APPLE_CERTIFICATE_PASSWORD` | Password protecting that export |
| `APPLE_SIGNING_IDENTITY` | Full Developer ID Application signing identity |
| `APPLE_API_KEY_P8` | Base64 App Store Connect notarization API private key |
| `APPLE_API_KEY_ID` | API key identifier |
| `APPLE_API_ISSUER` | API issuer identifier |

Create minisign keys with its official key generation tool. Create/export Sparkle keys with the `generate_keys` tool in the pinned Sparkle distribution. Keep offline backups of both private keys. Rotate keys by first publishing a version that trusts the new key, then signing subsequent releases with it. Never commit secrets. Sparkle's ZIP signing verifies that the private key matches the public key embedded in the package; JSON signing verifies against the compiled public keys before release.

## Release pipeline

1. Set `crates/alacrterm/Cargo.toml` to the new stable version and commit it.
2. Push a matching `v<major>.<minor>.<patch>` tag. Other tags fail validation; prerelease channels are not implemented yet.
3. Validate keys/certificates are configured, run formatting, strict Clippy and three-platform tests.
4. Build installers. macOS imports temporary signing credentials, signs native binaries and Sparkle components, notarizes/staples the app, and creates DMG plus `.app.zip`; Sparkle signs the final ZIP. Temporary credentials are removed even on failure.
5. Generate release notes, minisign-authenticated `update.json`, and `appcast.xml` with Sparkle's ZIP signature and immutable version-specific URLs.
6. Upload all files into a draft GitHub Release, then publish it as latest. A failed upload cannot expose a partial update feed. An interrupted draft must be reviewed/removed before retrying publication.

The stable feed URLs are `https://github.com/zephyraluco/alacrterm/releases/latest/download/update.json` and the equivalent `appcast.xml`. Future releases must preserve them and the bundle identifier `com.alacrterm.app` / Windows Inno `AppId=Alacrterm`.

## Local checks and native acceptance

Run:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked -p updater -p alacrterm-update-helper -p alacrterm -p terminal_view
cc -Wall -Wextra -Werror crates/updater/native/sparkle_state_test.c -o /tmp/sparkle-state-test
/tmp/sparkle-state-test
python -m unittest discover -s scripts/test -p 'test_*.py'
```

For a macOS build, run `python3 scripts/packaging/fetch_sparkle.py`, set `SPARKLE_FRAMEWORK_DIR` to the absolute `target/sparkle` directory, then build. The packaging script consumes `SPARKLE_PUBLIC_KEY` and optional Apple signing/notarization environment variables. A non-bundled development process is not an update target.

Before distributing the first update-enabled version, perform two-version upgrade drills on Windows and macOS with real signing credentials. Verify spaces in paths, multiple app instances, active transfers, unsaved previews, offline/truncated/corrupt packages, rejected authorization, installer failure, relaunch version confirmation, and preservation of user configuration. Linux tests verify shared code and installer command selection; they cannot validate Inno Setup, AppKit/Sparkle, code signing, notarization or an actual privileged deb installation.
