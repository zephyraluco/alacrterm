<h1 align="center">Alacrterm</h1>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">Local terminals, SSH sessions, file management, and X11 forwarding in one workspace.</p>

<p align="center">
  <a href="https://github.com/zephyraluco/alacrterm/actions/workflows/test.yml">
    <img src="https://github.com/zephyraluco/alacrterm/actions/workflows/test.yml/badge.svg" alt="Test">
  </a>
  <a href="https://github.com/zephyraluco/alacrterm/actions/workflows/package.yml">
    <img src="https://github.com/zephyraluco/alacrterm/actions/workflows/package.yml/badge.svg" alt="Package">
  </a>
  <a href="https://github.com/zephyraluco/alacrterm/releases">
    <img src="https://img.shields.io/github/v/release/zephyraluco/alacrterm?logo=github" alt="GitHub release">
  </a>
  <a href="https://github.com/zephyraluco/alacrterm/stargazers">
    <img src="https://img.shields.io/github/stars/zephyraluco/alacrterm?style=flat&logo=github" alt="GitHub stars">
  </a>
</p>

<p align="center">
  <a href="https://github.com/zephyraluco/alacrterm/releases">Download</a> ·
  <a href="docs/architecture.md">Architecture (Chinese)</a> ·
  <a href="https://github.com/zephyraluco/alacrterm/issues">Report an issue</a>
</p>

Alacrterm is a desktop terminal built with Rust, GPUI, and gpui-kit, with terminal emulation powered by Alacritty. Organize local shells and remote connections in tabs and split panes, browse files in the sidebar, and run remote graphical applications through a bundled X Server.

- **Tabs and splits:** Drag tabs to arrange terminal groups, resize panes, and use Zoom to focus on the current group.
- **Local and remote sessions:** Open a local terminal or a saved SSH session from the tab bar's `+` menu. Organize saved connections in folders.
- **Command completion:** Get suggestions from commands, history, arguments, paths, and project context, including local and remote path lookups.
- **File management:** Browse local or SFTP directories, navigate with path suggestions, preview and edit files, and upload or download them.
- **X11 forwarding:** Manage a bundled WayX X Server and connect SSH X11 channels to the local display service.
- **Themes and keybindings:** Switch UI themes and terminal palettes, reload external theme resources, and customize keyboard shortcuts.
- **Online updates:** Update through an installer on Windows, Sparkle on macOS, or the system's deb installation tools on Linux.

## Getting started

### Install

Download the package for your platform from [GitHub Releases](https://github.com/zephyraluco/alacrterm/releases).

| Platform | Current package target | Installation |
| --- | --- | --- |
| Windows | x64 | Run `*-setup.exe`; installs for the current user by default |
| macOS | Apple Silicon / ARM64 | Open the `.dmg` and drag Alacrterm into Applications |
| Linux | x64, Ubuntu 24.04 or compatible | Install the `.deb` with your system package manager |

On Linux, replace the filename below with the package you downloaded:

```sh
sudo apt install ./alacrterm-<version>-linux-x64.deb
```

If a release is not yet available, or you want to try a branch build, download installer artifacts from a successful [Package Actions](https://github.com/zephyraluco/alacrterm/actions/workflows/package.yml) run. Artifacts are retained for seven days. Windows installers are currently not Authenticode signed.

### Open a terminal or connection

1. Launch the app, click `+` in the tab bar, and choose a local terminal.
2. Create and save an SSH connection in the sessions sidebar, then open it from the sidebar or the `+` menu.
3. Drag tabs to create splits. Use the Zoom button to expand or restore the current terminal group.
4. Use the bottom status bar to toggle side panels, browse files for the current session, or view transfers.

The first connection to an unknown SSH host requires host key confirmation. X11 forwarding also requires the remote SSH server to allow forwarding and the remote environment to support the graphical application you want to run.

### Update

Check for updates in Settings, or click the update entry in the normal-mode title bar to review the current and available versions before installing. macOS uses Sparkle's native update UI. Linux hands the deb package to a system installation tool; restart the app manually after installation.

## Configuration

Release builds keep user configuration separate from installation files:

| Platform | User configuration directory |
| --- | --- |
| Windows | `%APPDATA%\Alacrterm` |
| macOS | `~/Library/Application Support/Alacrterm` |
| Linux | `$XDG_CONFIG_HOME/alacrterm`, defaulting to `~/.config/alacrterm` |

Configuration covers app preferences, terminal settings, saved sessions, custom keybindings, and update preferences. Debug builds use `config/` at the repository root; this directory is ignored by Git and excluded from installers.

Theme resources live in `themes/global` and `themes/terminal`. Runtime logs are retained for seven days by default. See the [architecture document](docs/architecture.md#配置资源与日志) for log locations and `RUST_LOG` behavior.

## Development

### Prerequisites

- Rust stable, including `rustfmt` and `clippy`.
- A native C/C++ build toolchain.
- Python 3.11+ for packaging and Python regression tests.
- On Windows, the MSVC toolchain; Inno Setup 6 is also required to create installers.
- On macOS, Xcode Command Line Tools; builds with Sparkle integration also need the framework prepared first.

Install native dependencies on Ubuntu 24.04, matching CI:

```sh
sudo apt-get update
sudo apt-get install -y cmake clang libfontconfig1-dev libfreetype-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libegl1-mesa-dev \
  libvulkan-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libssl-dev libasound2-dev
```

### Run from source

```sh
git clone https://github.com/zephyraluco/alacrterm.git
cd alacrterm
cargo run --locked -p alacrterm
```

Run on a machine with a desktop display. Platform runtime resources are included in the repository's `plugins/` directory; development builds can find them without copying them beside the executable.

To compile Sparkle integration on macOS, run these commands before building:

```sh
python3 scripts/packaging/fetch_sparkle.py
export SPARKLE_FRAMEWORK_DIR="$PWD/target/sparkle"
```

Online updates target installed release builds. Debug builds do not perform online update checks.

### Checks and packaging

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked -p alacrterm -p terminal_view -p updater -p alacrterm-update-helper -p util -p settings -p session -p transfer -p platform
cargo test --locked -p ssh --lib --test roundtrip
python -m unittest discover -s scripts/test -p 'test_*.py'
```

Create an installer for the current platform:

```sh
cargo build --release --locked -p alacrterm -p alacrterm-update-helper
python scripts/packaging/package.py
```

macOS packaging requires Sparkle and the framework path configured above. Output goes to `dist/`. See the [packaging guide](scripts/packaging/README.md) for platform dependencies, signing, and release requirements.

### Project layout

| Path | Contents |
| --- | --- |
| `crates/alacrterm` | App entry, windows, layout, sidebars, and application interactions |
| `crates/terminal` / `crates/terminal_view` | Terminal core, input, and rendering |
| `crates/ssh` / `crates/platform` | SSH, SFTP, X11, and platform runtimes |
| `crates/settings` / `crates/session` / `crates/transfer` | Configuration, saved sessions, and transfer state |
| `crates/updater` / `crates/update_helper` | Online updates and the standalone Windows installation helper |
| `crates/util` | Shared utilities, resource management, and logging |
| `assets` / `themes` / `plugins` | Icons, keymap presets, themes, and platform plugins |
| `scripts/packaging` / `scripts/test` | Packaging, release scripts, and regression tests |

See the [architecture document](docs/architecture.md) (Chinese) for module boundaries, data flow, and lifecycle constraints.

## Feedback and contributions

Use [Issues](https://github.com/zephyraluco/alacrterm/issues) to report bugs or suggest features. Include your operating system, app version, reproduction steps, and relevant screenshots or redacted logs.

Run the relevant checks before submitting code. For changes to window interactions or platform integrations, describe which platforms and operations you tested.

## Acknowledgments

- [Zed](https://github.com/zed-industries/zed): Reference for terminal view implementation and interaction design.
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and [gpui-kit](https://github.com/longbridge/gpui-kit): Windowing, rendering, and UI components.
- [Alacritty](https://github.com/alacritty/alacritty): Terminal emulation core.

## License

Alacrterm is distributed under [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0).

[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and [gpui-kit](https://github.com/longbridge/gpui-kit) are likewise licensed under Apache-2.0.
