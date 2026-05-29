# sync-clipboard

[简体中文](README.zh.md)

A personal tool to synchronize clipboard (text & images) between multiple computers on a local network.

> ⚠️ **Disclaimer:** This is a personal project written with **vibe coding**. It is not intended to be production‑grade, secure, or broadly compatible. Use at your own risk.

## Supported Platforms

| Platform | Desktop Environment / Compositor | Status |
|----------|----------------------------------|--------|
| Linux    | Wayland (KDE)                    | ✅ Tested |
| Linux    | Wayland (other compositors)      | 🤕 Untested |
| Linux    | X11                              | ❌ Not supported |
| Windows  | Windows 10/11                    | ✅ Tested |
| macOS    | —                                | ❌ Not supported |

## Known Limitations

- **No encryption.** All clipboard data is transmitted in plaintext over WebSocket. Only use on trusted local networks.
- **No authentication.** Any machine that can reach your `--listen` port can push clipboard content.
- **No support for platforms other than Windows and Linux Wayland.** No plans to add more (this is a personal project).

## Usage

```
Usage: sync-clipboard [OPTIONS]

Options:
  -l, --listen <LISTEN>    Address to listen on [default: 0.0.0.0:9000]
  -c, --connect <CONNECT>  Remote node to connect to (can be specified multiple times)
  -v, --verbose            Enable debug logging
  -h, --help               Print help
```

### Basic Examples

**Machine A (192.168.1.10) only listens, does not actively connect:**
```bash
./sync-clipboard --listen 0.0.0.0:9000
```

**Machine A (192.168.1.10):**
```bash
./sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.20:9000
```

**Machine B (192.168.1.20):**
```bash
./sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.10:9000
```

**Connect multiple nodes:**
```bash
./sync-clipboard --listen 0.0.0.0:9000 -c 192.168.1.10:9000 -c 192.168.1.30:9000
```

`--listen` can be `0.0.0.0:9000` (accept connections on all network interfaces) or bound to a specific IP. `--connect` can be specified multiple times to connect to several nodes. Auto‑reconnection is handled on disconnect.

## Auto‑start on Boot

### Linux (systemd user service)

Create a user‑level service file:

```bash
mkdir -p ~/.config/systemd/user
```

**`~/.config/systemd/user/sync-clipboard.service`:**
```ini
[Unit]
Description=Clipboard Sync
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=/home/yourusername/path/sync-clipboard \
    --listen 0.0.0.0:9000 \
    --connect 192.168.1.10:9000
Restart=always
RestartSec=5
StandardOutput=journal
# Wayland environment variables — not set by default in the systemd service environment
Environment=WAYLAND_DISPLAY=wayland-0
Environment=XDG_RUNTIME_DIR=/run/user/%U

[Install]
WantedBy=graphical-session.target
```

First verify your Wayland display name (usually `wayland-0` or `wayland-1`):

```bash
echo $WAYLAND_DISPLAY
```

If the output is not `wayland-0`, change `wayland-0` in the service file to the actual value.

Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now sync-clipboard.service

# View logs
journalctl --user -u sync-clipboard.service -f
```

> **Note:** The service depends on `graphical-session.target` because Wayland clipboard operations require an active compositor session. `%U` expands to the current user's UID (e.g., `1000`), so no manual adjustment is needed for `XDG_RUNTIME_DIR`.

### Windows (Task Scheduler)

1. Press `Win + R`, type `taskschd.msc`, and press Enter.
2. Click **Create Task** (not "Create Basic Task").
3. **General tab:**
   - Name: `Clipboard Sync`
   - Check **Run whether user is logged on or not** (or select "Run only when user is logged on" if you want to see the console window).
4. **Triggers tab:**
   - Click **New...**
   - Begin the task: **At log on**
   - (Optional) Add another trigger: **At startup**, with a 10‑second delay.
5. **Actions tab:**
   - Click **New...**
   - Action: **Start a program**
   - Program/script: `C:\path\to\sync-clipboard.exe`
   - Arguments: `--listen 0.0.0.0:9000 --connect 192.168.1.10:9000`
6. **Conditions tab:**
   - If using a laptop, uncheck "Stop if the computer switches to battery power" (or the equivalent option).
7. Click **OK** and enter your password to confirm.

You can verify by rebooting or by manually running the task in Task Scheduler.

## How It Works

Each machine runs an identical `sync-clipboard` instance, forming a **peer mesh** over WebSocket — no central server. Every node listens for incoming connections and can also actively connect to other nodes. When your local clipboard changes, the content is compressed (zstd) and broadcast to all connected peers. An echo‑suppression mechanism using SHA‑256 hashing + an 800 ms window prevents infinite loops.

Both **text** and **image** clipboard content are supported. On Windows, images are handled via `CF_BITMAP` / `CF_DIB`; on Linux, via the Wayland data‑device protocol.

## Requirements

- **Rust** (stable toolchain, 1.85+ recommended)
- Linux: A Wayland compositor (KDE Plasma, Sway, etc.)
- Network connectivity between machines (same LAN)

## Building

```bash
# Clone the repository
git clone git@github.com:ThriceCola/sync-clipboard.git
cd sync-clipboard

# Build (Linux Or Windows)
cargo build --release

# Build (Windows, cross‑compile from Linux)
# Requires: cargo-xwin + x86_64-pc-windows-msvc target
make xwin
```

The compiled binary is located at `target/release/sync-clipboard` (Linux) or `target/x86_64-pc-windows-msvc/release/sync-clipboard.exe` (Windows).

# Thanks

[wayland-clipboard-listener](https://github.com/Decodetalkers/wayland-clipboard-listener)

[clipboard-win](https://github.com/DoumanAsh/clipboard-win)
