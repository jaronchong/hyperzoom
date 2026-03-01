# HyperZoom

A cross-platform, peer-to-peer real-time video conferencing application written in Rust.

HyperZoom enables low-latency audio and video communication over direct UDP connections, with support for multi-participant sessions, automatic session recording, and UPnP NAT traversal.

## Features

- **Peer-to-peer conferencing** — Direct UDP communication, no server required
- **Multi-participant sessions** — Host accepts multiple guests (up to 255 peers)
- **High-quality audio** — Opus codec at 48 kHz with forward error correction
- **VP8 video** — 480p @ 30fps with adaptive bitrate (~400 kbps)
- **Local session recording** — Audio recorded to MP4 (AAC-LC) with session metadata
- **Adaptive jitter buffer** — Automatic depth adjustment for smooth playback
- **UPnP NAT traversal** — Automatic port mapping when available
- **Cross-platform** — macOS (Apple Silicon & Intel) and Windows

## Architecture

```
Camera → RGB → VP8 Encode → Fragment → UDP Send
                                          ↓
                              UDP Receive → Reassemble → VP8 Decode → Display

Microphone → Opus Encode → UDP Send
                              ↓
               UDP Receive → Jitter Buffer → Opus Decode → Speaker

Input Audio → AAC Encode → MP4 Recording (local)
```

### Modules

| Module | Purpose |
|--------|---------|
| `app` | egui/eframe GUI — pre-call setup, in-call video grid, post-call summary |
| `video/` | Camera capture (nokhwa), VP8 encode/decode, frame conversion, UDP fragmentation |
| `audio/` | Device I/O (cpal), Opus codec, adaptive jitter buffer, AAC recording, RT thread priority |
| `net/` | Custom UDP protocol, control messages, session state, UPnP port mapping |
| `recording/` | Session directory management and metadata serialization |

### Network Protocol

Custom binary protocol with a 12-byte header:

| Field | Size | Description |
|-------|------|-------------|
| Version | 2 bits | Protocol version |
| Type | 5 bits | Audio, VideoKeyframe, VideoDelta, Control, Bye |
| Participant ID | 1 byte | Sender identifier |
| Sequence | 2 bytes | Packet ordering |
| Timestamp | 4 bytes | Millisecond timestamp |
| Payload length | 2 bytes | Payload size |
| Fragment ID/Total | 2 bytes | Fragmentation for large video frames |

## Building

### Prerequisites

**Rust** (stable toolchain):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**macOS:**
```bash
brew install libvpx opus fdk-aac pkg-config
```

**Windows (vcpkg):**
```powershell
git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
C:\vcpkg\bootstrap-vcpkg.bat
C:\vcpkg\vcpkg install libvpx:x64-windows-static opus:x64-windows-static fdk-aac:x64-windows-static

# Set environment variables
$env:VPX_LIB_DIR = "C:\vcpkg\installed\x64-windows-static\lib"
$env:VPX_INCLUDE_DIR = "C:\vcpkg\installed\x64-windows-static\include"
$env:VPX_STATIC = "1"
$env:VPX_VERSION = "1.16.0"
$env:OPUS_LIB_DIR = "C:\vcpkg\installed\x64-windows-static"
$env:OPUS_INCLUDE_DIR = "C:\vcpkg\installed\x64-windows-static\include"
$env:OPUS_STATIC = "1"

# env-libvpx-sys expects libvpx.lib, but vcpkg creates vpx.lib
Copy-Item "$env:VPX_LIB_DIR\vpx.lib" "$env:VPX_LIB_DIR\libvpx.lib"
```

### Build & Run

```bash
cargo build --release
./target/release/hyperzoom
```

## Usage

1. **Host a session** — Enter your name, choose a port, and click Host
2. **Join a session** — Enter your name, the host's IP address and port, and click Join
3. **In-call** — Video grid displays all participants; toggle camera on/off
4. **After call** — View call summary and recording location

Recordings are saved to `~/HyperZoom/recordings/` with timestamped directories containing the audio MP4 and session metadata JSON.

## Platform Support

| Platform | Camera Backend | Status |
|----------|---------------|--------|
| macOS (Apple Silicon) | AVFoundation | Supported |
| macOS (Intel x86_64) | AVFoundation | Supported (cross-compiled) |
| Windows (x86_64) | Media Foundation (MSMF) | Supported |
| Linux | — | Audio only (no camera capture) |

## Dependencies

| Crate | Purpose |
|-------|---------|
| eframe / egui | Cross-platform GUI |
| cpal | Audio device I/O |
| nokhwa | Camera capture (AVFoundation / MSMF backends) |
| opus | Audio codec (low-delay, FEC) |
| vpx-encode / env-libvpx-sys | VP8 video codec |
| fdk-aac | AAC codec for local recording |
| tokio | Async networking runtime |
| igd-next | UPnP NAT traversal |
| fast_image_resize | Video frame scaling |
| ringbuf | Lock-free ring buffers for pipeline threads |

## Releases

Pre-built binaries are available on the [GitHub Releases](https://github.com/jaronchong/hyperzoom/releases) page for macOS (ARM64, x86_64) and Windows (x86_64).

## License

See [LICENSE](LICENSE) for details.
