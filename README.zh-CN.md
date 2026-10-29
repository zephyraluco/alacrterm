<h1 align="center">Alacrterm</h1>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">将本地终端、SSH 会话、文件管理和 X11 转发放在同一个工作区。</p>

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
  <a href="https://github.com/zephyraluco/alacrterm/releases">下载安装</a> ·
  <a href="docs/architecture.md">架构文档</a> ·
  <a href="https://github.com/zephyraluco/alacrterm/issues">问题反馈</a>
</p>

Alacrterm 是使用 Rust、GPUI 和 gpui-kit 构建的桌面终端，终端模拟基于 Alacritty。你可以在标签和分屏之间组织本地 Shell 与远程连接，在侧边栏浏览文件，并通过内置 X Server 运行远端图形程序。

- **标签与分屏**：拖动标签组织多个终端组，调整分屏比例；Zoom 模式放大当前组，专注于手头的终端。
- **本地与远程会话**：从标签栏 `+` 菜单启动本地终端或保存的 SSH 会话，以文件夹组织连接配置。
- **命令补全**：结合命令、历史、参数、路径和项目上下文提供候选，支持本地及远端路径查询。
- **文件管理**：浏览本地或 SFTP 目录，通过路径输入和候选快速跳转，预览、编辑文件并上传或下载。
- **X11 转发**：集成 WayX，管理本地 X Server，并将 SSH X11 信道连接到本地显示服务。
- **主题与按键**：切换界面主题和终端配色，热加载外部主题资源，自定义快捷键。
- **在线升级**：支持 Windows 安装器更新、macOS Sparkle 更新和 Linux deb 安装入口。

## 开始使用

### 下载安装

从 [GitHub Releases](https://github.com/zephyraluco/alacrterm/releases) 下载对应平台的安装包。

| 平台 | 当前打包目标 | 安装方式 |
| --- | --- | --- |
| Windows | x64 | 运行 `*-setup.exe`，默认安装到当前用户目录 |
| macOS | Apple Silicon / ARM64 | 打开 `.dmg`，将 Alacrterm 拖入 Applications |
| Linux | x64，Ubuntu 24.04 或兼容系统 | 使用系统包管理器安装 `.deb` |

Linux 安装示例，将文件名替换为实际下载的文件：

```sh
sudo apt install ./alacrterm-<版本>-linux-x64.deb
```

尚无正式发布或需要体验分支版本时，可从 [Package Actions](https://github.com/zephyraluco/alacrterm/actions/workflows/package.yml) 的成功运行中下载安装包产物，保留时间为七天。Windows 安装器目前未做 Authenticode 签名。

### 创建终端与连接

1. 启动应用，点击标签栏的 `+`，选择本地终端。
2. 在会话侧边栏创建并保存 SSH 连接，再从会话列表或 `+` 菜单打开。
3. 拖动标签创建分屏，使用 Zoom 按钮放大或还原当前终端组。
4. 通过底部状态栏切换侧边面板，浏览当前会话文件或查看传输任务。

首次连接未知 SSH 主机时，需要确认主机密钥。X11 转发还需要远端 SSH 服务允许该功能，并具备运行相应图形程序所需的环境。

### 更新应用

在设置的更新页检查新版本，或点击普通模式标题栏中的“可升级”按钮，查看当前版本和可用版本，再选择“立刻更新”。macOS 使用 Sparkle 原生更新界面；Linux 通过系统安装工具处理 deb，安装后手动重启应用。

## 配置

发布版本将用户配置与安装文件分开保存：

| 平台 | 用户配置目录 |
| --- | --- |
| Windows | `%APPDATA%\Alacrterm` |
| macOS | `~/Library/Application Support/Alacrterm` |
| Linux | `$XDG_CONFIG_HOME/alacrterm`，默认 `~/.config/alacrterm` |

配置包括应用偏好、终端设置、保存的会话、自定义按键和更新偏好。Debug 构建使用仓库根目录下的 `config/`；该目录不提交仓库，也不打进安装包。

主题资源位于 `themes/global` 和 `themes/terminal`。运行时日志默认保留七天，路径和 `RUST_LOG` 用法见[架构文档](docs/architecture.md#配置资源与日志)。

## 开发

### 环境准备

- Rust stable，包含 `rustfmt` 和 `clippy`。
- 本机 C/C++ 构建工具链。
- Python 3.11+，用于打包和 Python 回归测试。
- Windows 使用 MSVC 工具链，生成安装器还需要 Inno Setup 6。
- macOS 使用 Xcode Command Line Tools；集成 Sparkle 的构建需先准备框架。

Ubuntu 24.04 的原生依赖与 CI 一致：

```sh
sudo apt-get update
sudo apt-get install -y cmake clang libfontconfig1-dev libfreetype-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libegl1-mesa-dev \
  libvulkan-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libssl-dev libasound2-dev
```

### 从源码运行

```sh
git clone https://github.com/zephyraluco/alacrterm.git
cd alacrterm
cargo run --locked -p alacrterm
```

在具有桌面显示环境的本机运行。仓库中的 `plugins/` 提供各平台运行资源，开发时不必手动复制到可执行文件旁。

macOS 如需编译 Sparkle 集成，在构建前执行：

```sh
python3 scripts/packaging/fetch_sparkle.py
export SPARKLE_FRAMEWORK_DIR="$PWD/target/sparkle"
```

在线升级面向发布应用，Debug 构建不执行在线更新检查。

### 检查与打包

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked -p alacrterm -p terminal_view -p updater -p alacrterm-update-helper -p util
python -m unittest discover -s scripts/test -p 'test_*.py'
```

生成当前平台的安装包：

```sh
cargo build --release --locked -p alacrterm -p alacrterm-update-helper
python scripts/packaging/package.py
```

macOS 打包需要先准备 Sparkle 并设置上述框架路径。产物输出到 `dist/`；各平台依赖、签名和发布要求见[打包说明](scripts/packaging/README.md)。

### 项目结构

| 路径 | 内容 |
| --- | --- |
| `crates/alacrterm` | 应用入口、窗口、布局、侧边栏及业务交互 |
| `crates/terminal` / `crates/terminal_view` | 终端核心、输入与渲染 |
| `crates/ssh` / `crates/platform` | SSH、SFTP、X11 和平台运行时 |
| `crates/settings` / `crates/session` / `crates/transfer` | 配置、保存的会话与传输状态 |
| `crates/updater` / `crates/update_helper` | 在线升级及 Windows 独立安装 helper |
| `crates/util` | 通用工具、资源管理与日志 |
| `assets` / `themes` / `plugins` | 图标、按键预设、主题和平台插件 |
| `scripts/packaging` / `scripts/test` | 打包发布脚本与回归测试 |

模块边界、数据流和生命周期约束见[架构文档](docs/architecture.md)。

## 反馈与贡献

通过 [Issues](https://github.com/zephyraluco/alacrterm/issues) 提交问题或功能建议。报告问题时请附上操作系统、应用版本、复现步骤，以及必要的截图或脱敏日志。

提交代码前运行相关检查；涉及窗口交互或平台集成时，也请说明实际验证的平台与操作。

## 致谢

- [Zed](https://github.com/zed-industries/zed)：终端视图实现与交互设计参考。
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) 与 [gpui-kit](https://github.com/longbridge/gpui-kit)：窗口、渲染与界面组件。
- [Alacritty](https://github.com/alacritty/alacritty)：终端模拟核心。

## 许可证

Alacrterm 以 [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) 发布。

使用的 [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) 与 [gpui-kit](https://github.com/longbridge/gpui-kit) 同为 Apache-2.0 许可。
