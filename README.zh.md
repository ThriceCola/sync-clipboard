# sync-clipboard

一个在局域网内多台电脑之间同步剪贴板（文本 & 图片）的个人工具。

> ⚠️ **声明：** 这是用 **vibe code** 写出来的个人项目，纯粹为了满足自己的需求。不追求生产级质量、安全性或广泛兼容性。使用风险自负。

## 支持的平台

| 平台 | 桌面环境 / 合成器 | 状态 |
|------|------------------|------|
| Linux | Wayland（KDE） | ✅ 已测试 |
| Linux | Wayland（其他合成器） | 🤕未测试 |
| Linux | X11 | ❌ 不支持 |
| Windows | Windows 10/11 | ✅ 已测试 |
| macOS | — | ❌ 不支持 |

## 已知限制

- **无加密。** 所有剪贴板数据通过 WebSocket 明文传输。仅限在可信的局域网内使用。
- **无身份验证。** 任何能连接到你的 `--listen` 端口的机器都可以推送剪贴板内容。
- **不支持 Windows 与 Linux Wayland 以外的平台。** 暂无支持计划（这是个人项目）。

## 使用方法

```
用法: sync-clipboard [选项]

选项:
  -l, --listen <LISTEN>    监听的地址 [默认: 0.0.0.0:9000]
  -c, --connect <CONNECT>  要连接的远程节点（可多次指定）
  -v, --verbose            启用调试日志
  -h, --help               打印帮助信息
```

### 基本示例

**机器 A（192.168.1.10）只等待连接, 不主动连接：**
```bash
./sync-clipboard --listen 0.0.0.0:9000
```

**机器 A（192.168.1.10）上：**
```bash
./sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.20:9000
```

**机器 B（192.168.1.20）上：**
```bash
./sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.10:9000
```

**连接多个节点：**
```bash
./sync-clipboard --listen 0.0.0.0:9000 -c 192.168.1.10:9000 -c 192.168.1.30:9000
```

`--listen` 可以是 `0.0.0.0:9000`（接受所有网络接口的连接）或绑定到特定 IP。`--connect` 可多次指定以连接多个节点。断线后会自动重连。

## 开机自动启动

### Linux（systemd 用户服务）

创建用户级 service 文件：

```bash
mkdir -p ~/.config/systemd/user
```

**`~/.config/systemd/user/sync-clipboard.service`：**
```ini
[Unit]
Description=剪贴板同步
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=/home/你的用户名/路径/sync-clipboard \
    --listen 0.0.0.0:9000 \
    --connect 192.168.1.10:9000
Restart=always
RestartSec=5
StandardOutput=journal
# Wayland 环境变量 —— systemd 服务环境中默认没有这些，需要手动设置
Environment=WAYLAND_DISPLAY=wayland-0
Environment=XDG_RUNTIME_DIR=/run/user/%U

[Install]
WantedBy=graphical-session.target
```

先确认你的 Wayland display 名称（通常是 `wayland-0` 或 `wayland-1`）：

```bash
echo $WAYLAND_DISPLAY
```

如果输出不是 `wayland-0`，请把上面 service 文件里的 `wayland-0` 改成实际的值。

启用并启动：

```bash
systemctl --user daemon-reload
systemctl --user enable --now sync-clipboard.service

# 查看日志
journalctl --user -u sync-clipboard.service -f
```

> **注意：** 服务依赖于 `graphical-session.target`，因为 Wayland 剪贴板操作需要有活跃的合成器会话。`%U` 会自动展开为当前用户的 UID（如 `1000`），因此 `XDG_RUNTIME_DIR` 路径无需手动修改。

### Windows（任务计划程序）

1. 按 `Win + R`，输入 `taskschd.msc`，回车。
2. 点击**创建任务**（不是"创建基本任务"）。
3. **常规选项卡：**
   - 名称：`剪贴板同步`
   - 勾选**不管用户是否登录都要运行**（也可以选"只在用户登录时运行"，这样可以看到控制台窗口）。
4. **触发器选项卡：**
   - 点击**新建...**
   - 开始任务：**登录时**
   - （可选）再添加一个触发器：**启动时**，延迟 10 秒。
5. **操作选项卡：**
   - 点击**新建...**
   - 操作：**启动程序**
   - 程序或脚本：`C:\路径\sync-clipboard.exe`
   - 参数：`--listen 0.0.0.0:9000 --connect 192.168.1.10:9000`
6. **条件选项卡：**
   - 如果是笔记本，取消勾选"只有在计算机使用交流电源时才启动此任务"。
7. 点击**确定**，输入密码确认。

可以重启验证，或在任务计划程序中手动运行。



## 工作原理

每台机器运行一个相同的 `sync-clipboard` 实例，彼此通过 WebSocket 组成**对等网格**——没有中心服务器。每个节点都监听传入连接，同时也可以主动连接到其他节点。当你本地的剪贴板发生变化时，内容会被压缩（zstd）并广播给所有已连接的节点。通过 SHA-256 哈希 + 800ms 窗口的回声抑制机制来防止无限循环。

同时支持**文本**和**图片**剪贴板内容。Windows 端通过 `CF_BITMAP` / `CF_DIB` 处理图片，Linux 端则通过 Wayland data-device 协议。

## 环境要求

- **Rust**（稳定版工具链，推荐 1.85+）
- Linux：需要 Wayland 合成器（KDE Plasma、Sway 等）
- 机器之间网络互通（同一局域网）

## 编译

```bash
# 克隆仓库
git clone git@github.com:ThriceCola/sync-clipboard.git
cd sync-clipboard

# 编译（Linux Or Windows）
cargo build --release

# 编译（Windows，从 Linux 交叉编译）
# 需要: cargo-xwin + x86_64-pc-windows-msvc 目标
make xwin
```

编译产物位于 `target/release/sync-clipboard`（Linux）或 `target/x86_64-pc-windows-msvc/release/sync-clipboard.exe`（Windows）。


# 感谢

[wayland-clipboard-listener](https://github.com/Decodetalkers/wayland-clipboard-listener)

[clipboard-win](https://github.com/DoumanAsh/clipboard-win)
