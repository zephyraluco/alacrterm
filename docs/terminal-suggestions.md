# 终端历史建议：第一阶段

本阶段实现光标旁的候选列表和当前分屏的命令历史建议。参考 VS Code 将 shell 集成、
候选提供者和建议界面分离的设计；尚未实现 PATH 命令、路径、参数描述或灰色行内建议。

## 使用

输入已有历史命令的前缀，约 100 ms 后显示最多 8 条候选，最近执行的命令优先。
`Ctrl+Space` 手动触发，`↑` / `↓` 选择，`Tab` 或鼠标点击插入，`Esc` 关闭。
`Enter` 始终执行当前输入，不默认接受选中候选。插入候选不会附带回车。

历史仅保存在当前分屏内存中，最多 256 条，不读取或写入用户的历史文件。
只有用户提交且 shell 发出执行确认的命令才进入历史；以空白开头的命令不记录。
输入长度超过 4 KiB、多行粘贴、原生 Tab 补全或无法追踪的行编辑会停止本轮建议，
直到下一个已确认的新提示符。这样不会猜测 shell 实际输入或错误替换文字。

中文组合输入、全屏 TUI、鼠标报告模式、vi 模式、滚动查看历史、失焦和断线时不显示。
菜单靠近光标，空间不足时显示在上方，不超出当前分屏。

## Shell 集成

通过 OSC 633（兼容 OSC 133）的 A/B/C/D 生命周期标记判断提示符、输入、执行和结束，
不通过 `$`、`#` 或 `❯` 猜测。未集成的 shell 保留原生行为，不弹应用建议。

默认本地 Bash 4.4+、Zsh 和使用 PSReadLine 的 PowerShell 会加载会话临时集成脚本。
用户的 Bash rc、Zsh 配置及 PowerShell profile 保留，不修改任何启动文件；临时文件
随 PTY 生命周期清理。显式配置了自定义 shell 参数时保持原启动方式，不自动注入。
设置 `ALACRTERM_SHELL_INTEGRATION=0` 可禁止本地自动加载。

SSH 不会自动上传脚本或修改远端配置。将对应脚本复制到远端后，在交互会话中加载：

```sh
# Bash
source /path/to/bash.sh
# Zsh
source /path/to/zsh.zsh
```

PowerShell：

```powershell
. C:\path\to\powershell.ps1
```

脚本位于 `crates/terminal/shell-integration/`，本地使用的同份脚本嵌入应用。
SSH 已存在的 OSC 133/633 集成也可提供生命周期标记。当前追踪标准字符输入、
删除、左右移动及 Home/End、Ctrl+A/E；遇到历史导航、搜索或未知编辑操作时回退。

## 验证

- 状态机：执行确认、历史去重、Unicode 编辑、重绘标记、私密历史和安全回退。
- 扫描器：跨数据块 OSC、BEL/ST 结束符、未知序列及超长载荷恢复。
- Linux Bash/Zsh：用户配置加载与生命周期标记。
- Linux GUI：本地及实际 SSH 通道候选列表、键盘接受、Enter 执行和密码输入隔离。

Windows/macOS GUI 和 PowerShell 启动脚本尚需原生验证。
