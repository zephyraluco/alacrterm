# CI and installers

- `test`: every branch push and pull request; locked application and terminal-view tests. The terminal core's full test suite is not included (existing path/hyperlink failures).
- `package`: every branch push and pull request; Linux x64 deb (Ubuntu 24.04 or compatible), Windows x64 per-user Inno Setup installer, and macOS ARM64 dmg. Installer artifacts and SHA-256 files are retained for seven days.
- `release`: every tag push; calls the same packaging workflow at the tagged commit, then publishes its installers and checksums to GitHub Releases with generated release notes. Tags do not trigger the standalone test workflow.

The package version comes from `crates/alacrterm/Cargo.toml`; update it before tagging (recommended tag: `v<version>`). Build manually with `cargo build --release --locked -p alacrterm`, then `python scripts/packaging/package.py` on the matching OS/architecture. Python 3.11+ is required. Windows needs Inno Setup 6; macOS needs Xcode command-line tools; Linux needs dpkg-deb. Global themes and terminal palettes are installed beside the executable under `themes/global` and `themes/terminal`; bundled WayX runtime files and Windows ConPTY files are included.

macOS packages are ad-hoc signed, not Developer ID signed or notarized. Windows installers are not Authenticode signed. Publishing does not imply platform trust certification. Release publishing waits for all three packages to succeed.

Release-build user configuration (`app.json`, `terminal.json`, `sessions.json`, optional `keymap.json`) is stored exclusively in the system user configuration directory:

- Windows: `%APPDATA%\Alacrterm` (Roaming).
- Linux: `$XDG_CONFIG_HOME/alacrterm`, default `~/.config/alacrterm`.
- macOS: `~/Library/Application Support/Alacrterm`.

Debug/development builds instead read and write the repository-root `config/` directory, which is git-ignored and not packaged. There is no fallback between development and release configuration. Configuration directories are created on first save. Installation/source directories are not read for user configuration, and old files are not migrated. Bundled themes stay beside the executable.
