# HyperZoom — Product Requirements & Technical Specification

> **Version:** 0.1 MVP
> **Last updated:** 2026-02-27

---

## 1. Vision

HyperZoom is a peer-to-peer video call application purpose-built for **remote podcast recording**. Up to 4 people join a call that feels conversational and natural — ultra-low-latency audio so nobody talks over each other — while every participant simultaneously captures a **flawless 1080p local recording** of their own camera and microphone. After the session, participants exchange their "Master Golden Copies" offline, giving the editor broadcast-quality, frame-perfect multi-cam footage to cut a polished podcast.

**The core trade-off we embrace:** the live preview stream is intentionally ugly (480p, aggressive compression) so we can pour every microsecond of budget into keeping audio latency as low as physically possible. The beautiful footage is always being captured locally — it just isn't streamed.

---

## 2. Design Principles (ranked)

1. **Audio latency is king.** The live audio path must be the lowest-latency pipeline we can build. Every design decision that touches the live stream is evaluated against this. Target: sub-50ms mouth-to-ear one-way on a reasonable connection.
2. **Local recording is sacred.** The local 1080p + high-quality audio recording must never drop frames or audio samples, period. If the system is under pressure, degrade the live preview stream — never the local file.
3. **Simplicity for MVP.** Direct P2P over UDP. UPnP for port mapping. Manual IP exchange is fine. No STUN/TURN servers, no cloud infrastructure. Ship something that works for a small group of friends first.
4. **Performance through Rust.** Compiled native binaries. Zero-copy where possible. Lock-free ring buffers. Hardware-accelerated encoding. We use the language and OS facilities to their fullest.

---

## 3. Target Platforms

| Platform | Minimum version | Notes |
|---|---|---|
| macOS | 12 Monterey+ (Apple Silicon & Intel) | VideoToolbox for H.264 HW encode |
| Windows | 10 21H2+ / 11 | Media Foundation H.264 HW encode; NVENC as optional fast-path |

Single self-contained Rust binary per platform. Third-party C libraries (libopus, libvpx, fdk-aac) are statically linked into the binary. OS frameworks (VideoToolbox on macOS, Media Foundation on Windows) are dynamically linked at runtime as required by the OS. No installer required for MVP — just an executable.

---

## 4. Functional Requirements

### 4.1 Session Management

- **Host model:** One participant creates a session ("hosts"). The host's application opens the required UDP ports via UPnP on their router.
- **Join model:** Other participants (up to 3 guests) enter the host's public IP and port to connect. On join, each guest also opens their own UPnP port mapping so that other participants can send directly to them (required for full mesh).
- **Session metadata:** On connect, participants exchange a lightweight handshake containing: display name, supported codecs, stream parameters, public IP:port, and a session-unique participant ID.
- **Session capacity:** 2–4 participants. Full mesh topology — every participant sends to and receives from every other participant directly. All participants must have UPnP-capable routers.

### 4.2 Live Preview Stream (Real-Time)

The live stream exists solely so participants can see and hear each other with minimal delay. Quality is intentionally sacrificed for latency.

#### 4.2.1 Live Audio

| Parameter | Value | Rationale |
|---|---|---|
| Codec | Opus | Best low-latency audio codec available; designed for exactly this |
| Sample rate | 48 kHz | Opus native rate |
| Channels | Mono | Voice only — stereo adds latency and bandwidth for no benefit |
| Bitrate | 24–32 kbps CBR | Enough for clear speech, low enough to never congest |
| Frame size | 5 ms (or 2.5 ms if achievable) | Opus supports 2.5/5/10/20 ms frames. Smaller = lower latency |
| FEC | Enabled (Opus in-band FEC) | Recovers from occasional packet loss without retransmission |
| DTX | Enabled | Discontinuous transmission — saves bandwidth during silence |
| Jitter buffer | Adaptive, 5–30 ms ceiling | Start minimal, grow only if loss/jitter demands it |
| **Target one-way latency** | **< 50 ms** (codec + network + jitter buffer) | This is the single most important metric in the system |

#### 4.2.2 Live Video

| Parameter | Value | Rationale |
|---|---|---|
| Resolution | 480p (854x480) | Enough to see faces; cheap to encode |
| Codec | VP8 (software) | Fast encode, no HW dependency for preview stream; simple & battle-tested |
| Framerate | 24 fps | Adequate for talking heads; keeps bitrate down |
| Bitrate | 300–500 kbps VBR | Constrained to avoid competing with audio for bandwidth |
| Keyframe interval | Every 2 seconds | Fast recovery after packet loss without excessive bandwidth |
| **Priority** | **Always lower than audio** | If bandwidth is constrained, video quality/framerate degrades first |

#### 4.2.3 Transport — Live Stream

- **Protocol:** UDP with custom lightweight RTP-like framing.
- **Packet structure:** Each packet carries a header (participant ID, stream type, sequence number, timestamp) + payload. No SRTP for MVP (encryption is a post-MVP concern for LAN/trusted-network use).
- **Audio/video multiplexing:** Audio and video share the same UDP socket per peer pair, distinguished by stream-type flag in the header.
- **Congestion response:** Monitor RTT and packet loss. If loss exceeds threshold:
  1. Reduce video bitrate / framerate first.
  2. Reduce video resolution second (drop to 360p).
  3. Audio is **never** degraded — it always gets priority bandwidth.
- **No retransmission for audio.** Lost audio packets are gone. Opus FEC + PLC (packet loss concealment) handles gaps.
- **Selective retransmission for video keyframes only.** If a keyframe is lost, request retransmission via a lightweight NACK. Non-keyframe losses are tolerated (decoder will conceal).
- **Auto-resume on transient network loss.** If packets from a peer stop arriving but resume within the 5-second heartbeat timeout window, streams automatically re-sync without a full re-handshake. The call "heals" after a brief freeze. No new protocol mechanism is needed — the existing heartbeat timeout already defines the liveness window. The local recording is completely unaffected by network interruptions since it is an independent pipeline.

### 4.3 Local Recording (The Golden Master)

This is the entire point of the product. Every participant records their own camera and microphone locally at full quality, independently of the live stream. This file is the deliverable.

#### 4.3.1 Local Video Recording

| Parameter | Value | Rationale |
|---|---|---|
| Resolution | Camera's native resolution, up to 1920x1080 (1080p) | If the camera's max is 720p, record at 720p — upscaling adds no real quality. Actual resolution is documented in session_metadata.json. |
| Codec | H.264 (High Profile, Level 4.1) | Universal editing compatibility |
| Encoder | **Hardware-accelerated** — VideoToolbox (macOS), Media Foundation / NVENC (Windows) | CPU must remain free for live encode + decode |
| Framerate | 30 fps constant | CFR is mandatory — editors need frame-accurate timelines |
| Bitrate | 15–20 Mbps VBR | High enough for sharp 1080p; low enough that disk isn't a bottleneck |
| Keyframe interval | 1 second (GOP = 30 frames) | Makes editing / seeking fast without excessive file size |
| Color space | YUV 4:2:0 | Standard for H.264 delivery |
| Container | MP4 (fragmented / moov-at-end with recovery) | See §4.3.3 |

#### 4.3.2 Local Audio Recording

| Parameter | Value | Rationale |
|---|---|---|
| Codec | AAC-LC, 48 kHz, Mono | High compatibility with editors; better quality than MP3 at same bitrate |
| Bitrate | 192 kbps CBR | Overkill for voice — guarantees zero audible artifacts |
| Alternative | FLAC (lossless) as user-selectable option | For audiophile-grade archival; ~700 kbps for mono voice |
| Channels | Mono (mic input is mono; stored as mono or dual-mono) | Honest representation of source |
| **Zero-drop guarantee** | Audio capture runs on a dedicated high-priority thread with a lock-free ring buffer. Samples are never discarded. | This is non-negotiable. |

#### 4.3.3 Container & Crash Safety

- **Format:** Fragmented MP4 (fMP4) with periodic `moof`/`mdat` atoms flushed to disk.
- **Why:** If the app crashes or power is lost, the file is recoverable up to the last flushed fragment. Standard MP4 with `moov` at the end would be unrecoverable.
- **Fragment interval:** Every 1 second.
- **Finalization:** On clean session end, the app writes a final `moov` atom so the file is a standard-compatible MP4 playable everywhere.
- **Timestamp track:** Embed a timecode track (or wall-clock UTC timestamps in metadata) so that multi-participant recordings can be synchronized in post, even if participants started recording at slightly different times.

### 4.4 Synchronization Metadata

To make multi-cam editing possible, all participants must be time-aligned.

- **NTP-style time sync:** At session start, the host and each guest exchange a burst of timestamp packets to estimate clock offset (similar to NTP's algorithm but simplified). Each participant records this offset.
- **Embedded sync timecodes:** The local recording embeds the session-relative timestamp (host's clock as reference) so an editor can align all tracks by timecode alone.
- **Audio sync tone (standard):** At session start, the app plays a short sync tone (audible beep) that all participants record locally. This provides a sample-accurate sync point for editors, complementing the NTP-style timestamp sync. The two mechanisms cover each other's weaknesses: timestamps handle coarse alignment; the tone handles fine-grained, sample-accurate correction for any clock drift.

### 4.5 UI (MVP)

Minimal. Function over form. A single window.

- **Pre-call screen:** Camera preview, mic level meter, input device selection, "Host" or "Join" button. Displays estimated recording file size (e.g. "~8 GB for 1 hour at 1080p"). Checks available disk space and shows a warning if less than 20 GB free.
- **Headphone prompt:** On entering the pre-call screen, a brief modal: "HyperZoom requires headphones for best audio quality. Please connect headphones before joining." Dismissible (not a hard block), but makes the expectation explicit since there is no echo cancellation in MVP.
- **Host flow:** Click Host -> app opens UPnP ports -> displays public IP + port for sharing (user copies and sends via Discord/iMessage/etc.).
- **Join flow:** Click Join -> enter host IP:port -> connect.
- **In-call screen:**
  - Grid of participant video feeds (480p preview streams). 2x2 grid for 4 participants.
  - Mic mute/unmute toggle.
  - Camera on/off toggle.
  - Recording indicator (always on — recording starts automatically when the call begins).
  - Network stats overlay (toggle): per-peer RTT, packet loss %, audio latency estimate.
  - "End Call" button.
- **Post-call screen:** Shows path to the saved local recording file. "Open Folder" button.

---

## 5. Architecture

### 5.1 High-Level Data Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        LOCAL MACHINE                            │
│                                                                 │
│  ┌──────────┐    ┌──────────────────┐    ┌──────────────────┐   │
│  │  Camera   │───>│  Capture Thread  │───>│  Raw Frame Bus   │   │
│  │  (1080p)  │    │  (high priority) │    │  (lock-free ring)│   │
│  └──────────┘    └──────────────────┘    └────────┬─────────┘   │
│                                           ┌───────┴────────┐    │
│                                           │                │    │
│                                    ┌──────▼──────┐  ┌──────▼──────┐
│                                    │ LOCAL REC    │  │ LIVE ENCODE │
│                                    │ H.264 HW    │  │ VP8 480p    │
│                                    │ 1080p 30fps │  │ 24fps SW    │
│                                    └──────┬──────┘  └──────┬──────┘
│                                           │                │    │
│                                    ┌──────▼──────┐  ┌──────▼──────┐
│                                    │  MP4 Writer  │  │  UDP Send   │
│                                    │  (async I/O) │  │  Thread     │
│                                    └─────────────┘  └─────────────┘
│                                                                 │
│  ┌──────────┐    ┌──────────────────┐                           │
│  │   Mic    │───>│  Audio Capture   │──┬──────────────────────┐ │
│  │ (48kHz)  │    │  (RT priority)   │  │                      │ │
│  └──────────┘    └──────────────────┘  │                      │ │
│                                 ┌──────▼──────┐  ┌────────────▼┐│
│                                 │ LOCAL REC    │  │ LIVE ENCODE ││
│                                 │ AAC 192kbps  │  │ Opus 5ms   ││
│                                 │ (to MP4)     │  │ 24-32kbps  ││
│                                 └─────────────┘  └──────┬──────┘│
│                                                         │       │
│                                                  ┌──────▼──────┐│
│                                                  │  UDP Send   ││
│                                                  │  (HIGHEST   ││
│                                                  │   PRIORITY) ││
│                                                  └─────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Thread Model

| Thread | Priority | Responsibility | Notes |
|---|---|---|---|
| Thread | Priority | Runtime | Responsibility | Notes |
|---|---|---|---|---|
| **Audio Capture** | Real-time / highest | Raw OS thread | Pulls samples from OS audio device into ring buffers (x2) | Must never block. Uses OS real-time scheduling (macOS: `THREAD_TIME_CONSTRAINT_POLICY`, Windows: `MMCSS Pro Audio`). |
| **Audio Live Encode + Send** | Real-time / high | Raw OS thread | Reads from live SPSC ring, Opus-encodes, UDP-sends | Runs on every Opus frame boundary (5 ms). Combined to avoid an extra context switch. |
| **Audio Local Encode** | High | Raw OS thread | Reads from local SPSC ring, AAC-encodes, writes to MP4 muxer | Can tolerate slightly more latency; ring buffer absorbs jitter. |
| **Video Capture** | High | Raw OS thread | Pulls frames from camera API into frame ring buffers (x2) | OS camera callback thread; we copy out ASAP. |
| **Video Local Encode** | High | Raw OS thread | Takes raw frames, feeds to HW H.264 encoder, writes to MP4 muxer | HW encoder is async — submit frame, get packet back later. |
| **Video Live Encode** | Normal | Raw OS thread | Downscales to 480p, VP8-encodes, packetizes | Lower priority than local encode. If CPU is scarce, this drops frames first. |
| **Network Receive** | High | Tokio task | Polls UDP socket(s), demuxes incoming packets by peer + stream type | Dispatches audio to playback buffer, video to decode queue. |
| **Audio Playback** | Real-time / highest | Raw OS thread | Mixes decoded audio from all peers, pushes to OS audio output | Jitter buffer lives here. |
| **Video Decode** | Normal | Tokio task | Decodes incoming VP8 streams for display | One decode context per peer. |
| **Render / UI** | Normal | Main thread | Composites decoded video frames into window, handles UI events | Runs at display refresh or 30 fps, whichever is lower. |
| **MP4 Muxer / Disk I/O** | Normal | Tokio task | Interleaves and flushes encoded audio + video atoms to disk | Async I/O with buffering. Disk writes must not block encoders. |

### 5.3 Lock-Free Ring Buffers

Critical shared state between capture and encode threads uses **SPSC (single-producer, single-consumer) lock-free ring buffers**. Because each capture source feeds two independent consumers (live encoder + local recorder), the capture thread writes into **two separate SPSC rings per source**:

- **Audio rings (x2):** The audio capture thread pushes raw PCM samples into two independent SPSC rings — one for the Opus live encoder, one for the AAC local recorder. Each sized for ~200 ms of audio (a generous cushion).
- **Video rings (x2):** The video capture thread pushes raw frames (or GPU texture handles) into two independent SPSC rings — one for the VP8 live encoder, one for the H.264 HW local encoder. Each sized for 4–6 frames.

This avoids SPMC (single-producer, multi-consumer) complexity entirely. Each consumer has its own dedicated ring and progresses independently. If the live stream consumer falls behind, it skips to the latest data (drops intermediate frames). The local recording consumer must never fall behind — its ring buffer must be large enough to absorb any scheduling jitter. If it overflows, the application logs a critical warning but must not drop data.

### 5.4 Network Protocol

#### Packet Header (12 bytes)

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|V=1|P|  Type   |  Participant  |       Sequence Number         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         Timestamp (ms)                        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|         Payload Length        |   Fragment ID |  Fragment Tot |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

- **V (2 bits):** Protocol version = 1.
- **P (1 bit):** Padding flag.
- **Type (5 bits):** 0x01 = Audio, 0x02 = Video Keyframe, 0x03 = Video Delta, 0x04 = Control (handshake/NACK/stats/sync), 0x05 = BYE (graceful disconnect).
- **Participant (8 bits):** Sender's participant ID (0–3).
- **Sequence Number (16 bits):** Per-stream sequence, wrapping.
- **Timestamp (32 bits):** Milliseconds since session start. Overflows at ~49.7 days — effectively unlimited for any session length.
- **Payload Length (16 bits):** Bytes following the header.
- **Fragment ID / Fragment Total (8+8 bits):** For payloads that span multiple UDP packets (video keyframes). 0/0 = not fragmented.

#### Handshake Sequence

1. **Guest → Host:** `HELLO` packet (display name, supported codecs, version).
2. **Host → Guest:** `WELCOME` packet (session ID, assigned participant ID, list of other participants with their IPs).
3. **Host → All existing participants:** `PEER_JOINED` notification with new participant's info.
4. **All participants establish direct UDP connections to each other** (full mesh).
5. **Time sync exchange:** 8 round-trip timestamp pings to estimate clock offsets.
6. **Streams begin.**

#### Heartbeat & Disconnect

- **Heartbeat interval:** Every participant sends a heartbeat control packet to each peer every **1 second**.
- **Timeout:** If no packets (heartbeat, audio, or video) are received from a peer for **5 seconds**, that peer is considered disconnected. The UI displays a "disconnected" indicator and stops rendering their streams.
- **Graceful disconnect:** When a participant clicks "End Call", they send a `BYE` packet (type 0x05) to all peers before stopping streams. Peers immediately remove the participant without waiting for timeout.
- **BYE reliability:** The BYE packet is sent 3 times in rapid succession (at 50 ms intervals) to account for UDP loss. If all 3 are lost, the heartbeat timeout handles it within 5 seconds.

#### Bandwidth Budget (per peer pair, per direction)

| Stream | Bitrate | Notes |
|---|---|---|
| Audio (Opus) | 32 kbps | Fixed ceiling |
| Video (VP8 480p) | 300–500 kbps | Adapts down under congestion |
| Control/overhead | ~5 kbps | Heartbeats, NACKs, stats |
| **Total** | **~350–550 kbps** | Easily fits in most home upload bandwidth, even with 3 peers |

With 3 outgoing streams (full mesh, 4 participants), worst-case upload is ~1.65 Mbps — well within typical broadband.

---

## 6. Rust Crate / Dependency Strategy

| Concern | Crate / Library | Notes |
|---|---|---|
| Async runtime | `tokio` | For network I/O, timers, async file I/O **only** — never used for real-time audio/video threads (see §5.2) |
| Real-time threads | `std::thread` + platform FFI | Audio capture, audio playback, and audio live encode use raw OS threads with real-time scheduling. Not tokio tasks. |
| UDP networking | `tokio::net::UdpSocket` | Raw UDP — no framework overhead |
| Audio capture/playback | `cpal` | Cross-platform audio I/O |
| Video capture | `nokhwa` (or platform FFI) | Cross-platform camera; fall back to native APIs if needed |
| Opus encode/decode | `opus` (libopus binding) | Battle-tested, low-latency audio codec |
| VP8 encode/decode | `vpx` (libvpx binding) | Live preview video codec |
| H.264 HW encode | Platform-native FFI: VideoToolbox (macOS), Media Foundation (Windows) | Critical for offloading local recording to GPU |
| AAC encode | `fdk-aac` or platform-native | Local audio recording |
| MP4 muxing | `mp4` crate or custom fMP4 writer | Must support fragmented MP4 for crash safety |
| Lock-free rings | `ringbuf` or `crossbeam` | SPSC ring buffers |
| UPnP | `igd` crate | UPnP/IGD port mapping |
| GUI | `egui` + `eframe` (or `iced`) | Immediate-mode GUI — simple, fast, cross-platform |
| Image scaling | `fast_image_resize` | 1080p→480p downscale for live preview |

---

## 7. Local Recording File Layout

Each session produces a folder:

```
~/HyperZoom/recordings/
  └── 2026-02-27_20-15-00_SessionName/
      ├── local_recording.mp4          # Your 1080p H.264 + AAC recording
      ├── session_metadata.json         # Session info, participant list, clock offsets
      └── sync_timecodes.txt            # Human-readable timecode reference
```

### session_metadata.json

```json
{
  "session_id": "a1b2c3d4",
  "started_at_utc": "2026-02-27T20:15:00.000Z",
  "duration_seconds": 3842,
  "participants": [
    {
      "id": 0,
      "name": "Alice (Host)",
      "clock_offset_ms": 0
    },
    {
      "id": 1,
      "name": "Bob",
      "clock_offset_ms": -12340
    }
  ],
  "recording": {
    "video_codec": "H.264 High@4.1",
    "video_resolution": "1920x1080",
    "video_fps": 30,
    "video_bitrate_kbps": 18000,
    "audio_codec": "AAC-LC",
    "audio_sample_rate": 48000,
    "audio_bitrate_kbps": 192,
    "container": "fMP4",
    "total_frames_captured": 115260,
    "total_frames_dropped": 0
  }
}
```

---

## 8. Quality Assurance & Invariants

These are **hard invariants** that must hold in every build:

1. **Local recording never drops frames.** If the HW encoder queue backs up, the video ring buffer must be large enough to absorb it. If it can't, the application must log a critical warning — but still not drop.
2. **Local recording never drops audio samples.** The audio ring buffer must be large enough to absorb any scheduling jitter. Audio capture thread runs at real-time priority.
3. **Audio packets are sent before video packets.** When both are ready at the same instant, audio always goes out first.
4. **Live video degrades before audio.** Under congestion: reduce video bitrate → reduce video framerate → reduce video resolution → drop video entirely. Audio is never touched.
5. **Session end writes a valid MP4.** The finalization step must be robust. If it fails, the fragmented MP4 is still playable by ffmpeg/VLC.
6. **UPnP port mappings are cleaned up on exit.** All participants (host and guests) unmap their ports on clean shutdown. If the app crashes, mappings will expire via UPnP lease timeout.

---

## 9. Performance Targets

| Metric | Target | Measurement method |
|---|---|---|
| Audio one-way latency (mouth-to-ear) | < 50 ms on LAN, < 100 ms over internet | Loopback measurement with timestamp comparison |
| Audio capture-to-send latency | < 10 ms (codec frame + packetization) | Internal instrumentation |
| Video capture-to-display latency | < 150 ms | Acceptable — video latency is less critical |
| Local recording CPU overhead | < 15% of one core (HW encode) | Profiling; HW encoder does the heavy lifting |
| Live encode CPU overhead | < 50% of one core (VP8 software) | Profiling |
| Memory usage | < 300 MB | Mostly raw frame buffers |
| Application startup to camera-ready | < 3 seconds | Stopwatch |
| Binary size | < 30 MB | `strip` + LTO |

---

## 10. MVP Scope & Non-Goals

### In Scope (MVP)

- [x] P2P UDP full-mesh connectivity (up to 4 participants)
- [x] UPnP port opening for all participants (host + guests)
- [x] Manual IP:port join flow
- [x] Ultra-low-latency Opus audio streaming
- [x] 480p VP8 video preview streaming
- [x] 1080p H.264 hardware-accelerated local recording
- [x] AAC local audio recording
- [x] Fragmented MP4 container with crash recovery
- [x] NTP-style clock sync for multi-cam timecode alignment
- [x] Minimal egui-based UI
- [x] Mic mute / camera toggle
- [x] Auto-start recording on call join
- [x] Audio sync tone at session start for sample-accurate multi-cam alignment
- [x] Headphone prompt on pre-call screen
- [x] Pre-call disk space check and recording size estimate
- [x] Auto-resume live streams on transient network loss (within 5-second heartbeat window)
- [x] Heartbeat (1s interval) and BYE packet for disconnect detection

### Not in Scope (Post-MVP)

- End-to-end encryption (DTLS/SRTP)
- STUN/TURN relay for NAT traversal beyond UPnP
- Screen sharing
- Text chat
- Cloud recording or upload
- Multi-monitor support
- Virtual backgrounds
- Noise suppression / echo cancellation (rely on user headphones for MVP)
- Mobile platforms (iOS/Android)
- Auto-update mechanism
- Full session reconnection after extended network drop (>5 seconds) with re-handshake
- Custom resolution/bitrate settings UI

---

## 11. Post-Session Workflow (User Story)

1. Four friends hop on a HyperZoom call to record their weekly podcast.
2. They chat for 60 minutes. The conversation feels natural — audio latency is so low it's like being in the same room. The 480p video preview is grainy but good enough to read facial expressions and hand gestures.
3. Everyone hits "End Call." Each person now has a `local_recording.mp4` on their machine — pristine 1080p, crystal-clear audio, zero dropped frames.
4. They share files via Google Drive / Dropbox / WeTransfer / whatever.
5. The editor imports all four MP4 files into DaVinci Resolve or Premiere. The embedded timecodes let them snap all tracks into alignment instantly.
6. The final podcast is cut, exported, and published — and it looks and sounds professional because every frame came from a local, hardware-encoded master.

---

## 12. Open Questions

- **Echo cancellation:** MVP assumes headphones. If users want to use speakers, we'll need WebRTC-style AEC. Investigate `webrtc-audio-processing` crate for post-MVP.
- **Opus frame size:** 5 ms is the target, but 2.5 ms may be achievable if CPU allows. Needs benchmarking.
- **VP8 vs H.264 for live stream:** VP8 chosen for simplicity (software-only, no HW dependency). If HW can handle two simultaneous H.264 streams (local + live) on target hardware, H.264 might be better. Needs testing.
- **GUI framework:** `egui` is the current pick for speed of development. If we need more polished UI later, `iced` or platform-native UI are options.
- **FLAC vs AAC for local audio:** AAC is the default for editor compatibility. FLAC is lossless but less universally supported in NLEs. Offer as option?
