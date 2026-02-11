# Alacrterm

一个基于 Rust 和 GPUI 框架构建的现代化终端模拟器。

## 项目简介

Alacrterm 是一个高性能的跨平台终端模拟器，结合了 Alacritty 的终端核心能力和 GPUI 的现代化 UI 框架，旨在提供流畅、美观的终端使用体验。

## 核心特性

- **跨平台支持** - 支持 Windows、macOS 和 Linux
- **现代化 UI** - 基于 GPUI 框架构建的图形界面
- **主题系统** - 内置多个主题（Tokyo Night、Ayu），支持动态加载和热重载
- **进程管理** - 完整的 PTY 和进程信息追踪功能
- **异步架构** - 基于 Tokio 的高性能异步运行时
- **资源嵌入** - 80+ 个 SVG 图标编译到二进制文件中
- **模块化设计** - 清晰的 workspace 结构，便于维护和扩展

## 技术栈

| 技术 | 版本 | 用途 |
|------|------|------|
| Rust | Edition 2024 | 核心语言 |
| GPUI | 0.2 | GUI 框架 |
| Alacritty Terminal | 0.25 | 终端模拟核心 |
| Tokio | 1.x | 异步运行时 |
| Sysinfo | 0.38 | 系统和进程信息 |
| Rust-embed | 8 | 资源嵌入 |

## 项目结构

```
alacrterm/
├── src/                      # 主应用源代码
│   ├── main.rs              # 应用入口
│   ├── assets.rs            # 资源管理（80+ 图标）
│   ├── themes.rs            # 主题系统
│   └── layout.rs            # 布局定义
│
├── crates/
│   └── terminal/            # 终端核心库
│       ├── src/
│       │   ├── lib.rs       # 终端库主入口
│       │   ├── pty_info.rs  # PTY 进程信息管理
│       │   └── utils.rs     # 工具函数
│       └── examples/
│           └── demo.rs      # 终端演示示例
│
├── conpty/                  # Windows ConPTY 支持
├── assets/icons/            # SVG 图标集
├── themes/                  # 主题配置文件
│   ├── tokyonight.json
│   └── ayu.json
└── Cargo.toml              # 工作区配置
```

## 核心功能

### 终端模拟

基于 Alacritty 终端库实现，提供完整的终端模拟功能：
- 支持 80x24 标准终端尺寸
- 跨平台 PTY 管理
- 事件驱动架构

### 进程管理

- 实时进程信息追踪
- 工作目录监控
- 进程生命周期管理
- 跨平台进程 ID 获取（Windows/Unix）

### Shell 支持

- Windows: 优先使用 PowerShell 7 (pwsh.exe)
- Unix/Linux: 使用系统默认 Shell
- macOS: 支持 zsh/bash
