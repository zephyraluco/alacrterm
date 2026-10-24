# WayX / SSH X11 转发

`feat/x11-forwarding` 使用 `platform/` 中的 WayX 原生运行包，不依赖宿主的
XQuartz / VcXsrv。参考实现为 [zephyraluco/WayX](https://github.com/zephyraluco/WayX)
（适配参考提交 `f4de45c200f34b03b4c0d27167204a7f7a67d1c0`）；
cookie 校验逻辑的许可证见 [wayx-LICENSE](wayx-LICENSE)。

## 使用

Alacrterm 打开主窗口后在后台异步启动一个共享的 rootless WayX 服务，与是否打开
SSH 会话无关。通过本地无认证 X11 握手后，状态才变为运行中；启动不阻塞主窗口。

标题栏右上角的按钮仅显示状态图标，不显示文字或悬浮提示：已停止、启动中、运行中、
停止中、失败各有对应图标。启动中和停止中暂不接受重复点击。点击启动或停止会弹出
确认框，确认后执行，取消不改变服务状态。停止确认框说明图形窗口会关闭，而 SSH
终端和 SFTP 不受影响。启动失败或服务异常退出仅改变图标，不弹通知；可点击重试。
应用退出时同步终止服务并清理 socket、显示锁和临时目录。

所有 SSH 终端会话默认申请 X11 转发，不再提供或保存会话级开关。旧会话文件中的
`x11_forwarding` 字段被忽略，无论原值为 true 或 false 都使用新行为。服务端不支持、
拒绝或未及时确认 X11 请求时静默回退到普通 SSH，不显示 X11 错误提示。
服务器需要安装 `xauth`，并在 sshd 中启用 `X11Forwarding yes` 才能运行远端图形程序。
`DISPLAY` 由 SSH 服务端设置，不要手工设置成本地显示编号。

WayX 手动停止后，SSH 连接不会自动重启它。已经成功协商 X11 的会话保留独立的远端
cookie；每次打开图形连接时读取当前服务的显示编号。重新启动 WayX 后，
原 SSH 会话可再次运行 `xclock` 等程序，无需重连。之前关闭的图形应用需要重新运行。
每次启停更换服务代次，旧的启动任务不能覆盖新状态，旧的图形通道会关闭。
本地 WayX 显式使用 `-ac` 免认证模式，不生成 Xauthority 文件。SSH 会话仍生成并校验
独立的转发 cookie，校验成功后移除 X11 握手中的认证字段，再连接本地服务。
Unix 禁用 TCP，仅使用本地 socket；Windows 使用回环 TCP，不对局域网开放。
本机其他进程可以访问此免认证显示。
共享服务最多同时处理 64 条 X11 通道，初始认证限时 10 秒。

当前运行包支持 Linux x64、macOS arm64、Windows x64。发布时应将完整 `platform/`
目录放在应用可执行文件旁边（保留 Unix 启动器与 `bin/` 的可执行权限）。开发时也会
从当前目录及可执行文件的祖先目录查找 `platform/`。可以通过 `WAYX_RUNTIME_DIR`
指定单个运行包根目录，即包含 `bin/`、`share/` 和 Unix `lib/` 的目录；不支持的架构
必须提供对应的运行包。程序直接启动原生 `bin/wayx[.exe] serve`，不依赖 shell。

Linux 运行包要求 glibc 2.41 或更新版本，并需要宿主显示服务、GPU 驱动和字体。
macOS / Windows 的运行行为仍需相应平台实测。

## SSH 压缩

SSH 终端连接（包括其中的 X11 通道）默认按以下顺序协商压缩：
`zlib@openssh.com` → `zlib` → `none`。优先使用认证成功后启用的 OpenSSH 延迟压缩，
服务端仅支持普通 zlib 时使用普通压缩，不支持压缩时正常回退。独立的 SFTP 连接
使用相同策略；不需要额外配置 WayX。

压缩减少可压缩数据的网络流量，但会增加 CPU 开销，不能消除 X11 协议往返延迟，
也不等同于视频编码；实际效果取决于应用内容和网络条件。

## 验证

```sh
cargo check --locked --workspace
cargo test --locked -p ssh
cargo test --locked -p alacrterm sidebar_panel::sessions::tests
```

需要宿主 DISPLAY / Wayland 环境的测试单独运行：

```sh
cargo test --locked -p ssh --lib manual_lifecycle_restarts_without_ssh_and_cancels_stale_startup -- --ignored
cargo test --locked -p ssh --test roundtrip shared_x11_server_survives_disconnecting_another_session -- --ignored
```

第一个测试不创建 SSH 会话，直接验证启动、停止、重启和启动代次隔离。第二个测试
建立两条并发 SSH 会话，创建并映射图形窗口；关闭其中一个会话后，另一个继续运行，
停止 WayX 后仍可使用终端，重启后原会话可再次建立 X11 连接。默认测试还验证服务端
拒绝 X11 时保持交互式 shell，且没有 X11 状态提示。这些测试无需外部主机或凭据。
云环境可使用 `Xvfb :99 -screen 0 1280x800x24 -nolisten tcp -ac -extension MIT-SHM`
并给上述命令设置 `DISPLAY=:99`。
