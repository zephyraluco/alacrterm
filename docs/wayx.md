# WayX / SSH X11 转发

`feat/x11-forwarding` 使用 `platform/` 中的 WayX 原生运行包，不依赖宿主的
XQuartz / VcXsrv。参考实现为 [zephyraluco/WayX](https://github.com/zephyraluco/WayX)
（适配参考提交 `9a3bef2b3b1d99220ad3a4f134567d4ab5026a06`）；
cookie 校验逻辑的许可证见 [wayx-LICENSE](wayx-LICENSE)。

## 使用

新建 SSH 会话时勾选 **启用 X11 转发（WayX）**，连接后直接在远端终端运行
`xterm` 等 X11 应用。服务器需要安装 `xauth`，并在 sshd 中启用
`X11Forwarding yes`。`DISPLAY` 由服务器的 X11 转发请求设置，不要手工改成本地编号。
默认不启用转发，旧的会话文件也按关闭处理。

所有启用转发的终端会话共用一个 rootless WayX 进程和显示编号，首次连接时按需
启动并完成认证握手；并发连接只会启动一个进程。每个 SSH 会话仍使用独立的远端
cookie，共享服务使用同一个本地认证 cookie。会话断开只关闭自己的转发通道；即使
最后一个会话断开，服务也会保留，直到应用退出时回收进程、认证文件与临时目录。
如果服务意外退出，下一次连接会重新启动；已有会话需重新连接以恢复图形转发。
远端 X11 通道必须先通过独立的随机 cookie 校验，随后将 cookie 替换成本地的认证
cookie；本地 cookie 不发送给服务器。不启用转发时拒绝服务端打开的 X11 通道。
共享服务最多同时处理 64 条 X11 通道，初始认证限时 10 秒。

当前运行包支持 Linux x64、macOS arm64、Windows x64。发布时应将完整 `platform/`
目录放在应用可执行文件旁边（保留 Unix 启动器与 `bin/` 的可执行权限）。开发时也会
从当前目录及可执行文件的祖先目录查找 `platform/`。可以通过 `WAYX_RUNTIME_DIR`
指定单个运行包根目录，即包含 `bin/`、`share/` 和 Unix `lib/` 的目录；不支持的架构
必须提供对应的运行包。程序直接启动原生 `bin/wayx[.exe] serve`，不依赖 shell。

Linux 运行包要求 glibc 2.41 或更新版本，并需要宿主显示服务、GPU 驱动和字体。
macOS / Windows 的运行行为仍需相应平台实测。

## 验证

```sh
cargo check --locked --workspace
cargo test --locked -p ssh
cargo test --locked -p alacrterm sidebar_panel::sessions::tests
```

需要宿主 DISPLAY / Wayland 环境的测试单独运行：

```sh
cargo test --locked -p ssh --lib shared_wayx_reuses_server_and_is_reaped_on_shutdown -- --ignored
cargo test --locked -p ssh --test roundtrip shared_x11_server_survives_disconnecting_another_session -- --ignored
```

第一个测试确认多个会话复用同一个进程和显示编号、使用不同的远端 cookie，并验证
最后一个会话断开后服务仍然存在、应用关闭后资源被回收。第二个测试通过本机测试
SSH 服务端建立两条并发会话，认证请求经 cookie 替换后连接真实的内置 X server，
创建、映射窗口并完成请求往返；断开其中一个会话后，另一个仍能建立新的 X11 连接。
这些测试不需要远端主机或外部凭据。
云环境可使用 `Xvfb :99 -screen 0 1280x800x24 -nolisten tcp -ac -extension MIT-SHM`
并给上述命令设置 `DISPLAY=:99`。
