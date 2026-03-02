# HyperZoom Web — Product Requirements & Technical Specification

> **Version:** 0.2 (Browser-First Rewrite)
> **Last updated:** 2026-03-02
> **Predecessor:** [SPECS.md](./SPECS.md) (native P2P desktop MVP)

---

## 1. Vision

HyperZoom is a real-time video call application purpose-built for **remote podcast recording**. Up to 4 people join a call that feels conversational and natural — ultra-low-latency audio so nobody talks over each other — while every participant simultaneously captures a **flawless 1080p local recording** of their own camera and microphone. After the session, participants download their "Master Golden Copies" and hand them to an editor for broadcast-quality, frame-perfect multi-cam footage.

**This version reimagines HyperZoom as a browser-first application.** The entire client runs as a WebAssembly module inside any modern browser — desktop or mobile — with no install required. A lightweight Cloudflare-based server handles signaling and NAT relay, replacing the previous UPnP-only approach. The result is the same ultra-low-latency podcast recording experience, but accessible to anyone with a browser and a link.

**The core trade-off we embrace remains the same:** the live preview stream is intentionally modest (480p, aggressive compression) so we can pour every microsecond of budget into keeping audio latency as low as physically possible. The beautiful footage is always being captured locally — it just isn't streamed.

---

## 2. Design Principles (ranked)

1. **Audio latency is king.** The live audio path must be the lowest-latency pipeline we can build within browser constraints. Every design decision that touches the live stream is evaluated against this. Target: sub-100ms mouth-to-ear one-way over the internet; sub-150ms on mobile.
2. **Local recording is sacred.** The local 1080p + high-quality audio recording must never drop frames or audio samples, period. If the system is under pressure, degrade the live preview stream — never the local file.
3. **Zero install, universal access.** Opening a link in a browser is the entire onboarding flow. No downloads, no plugins, no extensions. Works on phones too.
4. **Cloudflare-native infrastructure.** The server stack runs entirely on Cloudflare's edge platform (Workers, Durable Objects, Pages, R2, D1). No VMs, no containers, no ops burden. The infrastructure scales to zero when idle and handles spikes automatically.
5. **Performance through WASM.** The performance-critical core (fMP4 muxer, jitter buffer, protocol logic, frame processing) is compiled Rust → WebAssembly. Browser APIs handle media capture and codec acceleration. We use each platform's strengths.

---

## 3. Target Platforms

### 3.1 Primary — Browser (WASM)

| Browser | Minimum Version | Notes |
|---|---|---|
| Chrome / Chromium | 94+ | Full WebCodecs, WebRTC, OPFS support |
| Edge | 94+ | Chromium-based; same capabilities as Chrome |
| Safari | 16.4+ | WebCodecs landed in 16.4; OPFS in 15.2 |
| Firefox | 130+ | WebCodecs (behind flag until 130); OPFS support |
| Chrome Android | 94+ | Mobile primary target; hardware WebCodecs |
| Safari iOS | 16.4+ | Mobile primary target; limited WebCodecs profiles |

**Required browser APIs:** WebRTC, WebCodecs, Web Audio API, MediaDevices (getUserMedia), Origin Private File System (OPFS), Web Workers, SharedArrayBuffer (with appropriate COOP/COEP headers).

### 3.2 Secondary — Desktop Native (via Tauri)

| Platform | Minimum Version | Notes |
|---|---|---|
| macOS | 12 Monterey+ (Apple Silicon & Intel) | Tauri uses WKWebView; WebCodecs via system WebKit |
| Windows | 10 21H2+ / 11 (x86_64) | Tauri uses WebView2 (Chromium-based); full WebCodecs |

The Tauri desktop app wraps the identical web client in a native window, adding:
- System tray integration and native notifications
- Direct filesystem access for recordings (no OPFS intermediary)
- Optional higher recording bitrates (unconstrained by browser storage quotas)
- Native window management and keyboard shortcuts

The desktop build is a **thin native shell around the same WASM core and web UI**. There is one codebase, not two.

### 3.3 Server — Cloudflare Edge

| Service | Role |
|---|---|
| **Cloudflare Pages** | Hosts the static web client (HTML, CSS, JS, WASM) |
| **Cloudflare Workers** | Signaling server, REST API, WebSocket upgrade handler |
| **Cloudflare Durable Objects** | Per-session state: participant list, signaling relay, session lifecycle |
| **Cloudflare KV** | Short-lived session tokens, room lookup by invite code |
| **Cloudflare D1** | Persistent data: session history, user preferences (post-MVP) |
| **Cloudflare R2** | Optional cloud backup storage for recordings (post-MVP) |
| **Cloudflare TURN** | TURN relay service for participants behind symmetric NATs |

---

## 4. Functional Requirements

### 4.1 Session Management

- **Create model:** One participant creates a session via the web UI. The Cloudflare Worker generates a unique room code (e.g., `hype-blue-fox-42`) and creates a Durable Object to manage the session.
- **Join model:** Other participants (up to 3 guests) navigate to a URL like `hyperzoom.app/hype-blue-fox-42` or enter the room code manually. No IP addresses, no port numbers.
- **Signaling:** All session control (join, offer/answer exchange, ICE candidates, participant events) flows through the Durable Object via WebSocket. The Durable Object acts as a lightweight signaling server.
- **Session metadata:** On connect, participants exchange a WebRTC handshake mediated by the signaling server, including: display name, supported codecs (negotiated via SDP), and a session-unique participant ID assigned by the server.
- **Session capacity:** 2–4 participants. Full mesh WebRTC topology — every participant establishes a direct peer connection to every other participant. The signaling server only mediates the initial connection; media flows P2P.
- **NAT traversal:** WebRTC ICE handles NAT traversal automatically using STUN (Cloudflare-provided or public STUN servers) and TURN relay (Cloudflare TURN service) as fallback for symmetric NATs.

### 4.2 Live Preview Stream (Real-Time)

The live stream exists solely so participants can see and hear each other with minimal delay. Quality is intentionally sacrificed for latency.

#### 4.2.1 Live Audio

| Parameter | Value | Rationale |
|---|---|---|
| Codec | Opus | WebRTC's mandatory-to-implement codec; designed for exactly this |
| Sample rate | 48 kHz | Opus native rate; Web Audio API default |
| Channels | Mono | Voice only — stereo adds latency and bandwidth for no benefit |
| Bitrate | 24–32 kbps CBR | Enough for clear speech, low enough to never congest |
| Frame size | 10 ms (browser-constrained) | WebRTC's Opus implementation typically uses 10–20 ms. We configure for minimum. |
| FEC | Enabled (Opus in-band FEC) | Recovers from occasional packet loss without retransmission |
| DTX | Enabled | Discontinuous transmission — saves bandwidth during silence |
| Jitter buffer | Adaptive, browser-managed + WASM-assisted | WebRTC handles primary jitter buffering; WASM assists with timing analysis |
| **Target one-way latency** | **< 100 ms** (internet), **< 150 ms** (mobile) | Browser adds ~20–40 ms overhead vs. native; still excellent for conversation |

**Note on latency vs. SPECS v1:** The native build targeted sub-50ms with raw UDP and 5ms Opus frames. The browser imposes additional latency from WebRTC's jitter buffer, audio worklet scheduling, and 10ms minimum Opus frames. We accept this trade-off for universal access. For users who need absolute minimum latency, the Tauri desktop build will approach native performance.

#### 4.2.2 Live Video

| Parameter | Value | Rationale |
|---|---|---|
| Resolution | 480p (854x480) | Enough to see faces; cheap to encode |
| Codec | VP8 or VP9 (WebRTC-negotiated) | Mandatory WebRTC codecs; hardware-accelerated decode on most devices |
| Framerate | 24 fps | Adequate for talking heads; keeps bitrate down |
| Bitrate | 300–500 kbps VBR | Constrained to avoid competing with audio for bandwidth |
| Keyframe interval | Every 2 seconds | Fast recovery after packet loss without excessive bandwidth |
| **Priority** | **Always lower than audio** | If bandwidth is constrained, video quality/framerate degrades first |

#### 4.2.3 Transport — Live Stream

- **Protocol:** WebRTC with P2P full mesh.
- **Audio transport:** WebRTC audio tracks with Opus. Configured via SDP for minimum latency (max-ptime, stereo=0, usedtx=1, useinbandfec=1).
- **Video transport:** WebRTC video tracks with VP8/VP9. Configured for 480p with bandwidth constraints via `setParameters()`.
- **Data channel:** A reliable, ordered DataChannel per peer pair for control messages (heartbeat, stats, sync metadata, recording state). Replaces the custom UDP control protocol from SPECS v1.
- **Congestion response:** WebRTC's built-in congestion control (GCC — Google Congestion Control) manages bandwidth estimation. We layer application-level adaptation on top:
  1. Reduce video bitrate / framerate first via `setParameters()`.
  2. Reduce video resolution second (drop to 360p).
  3. Audio is **never** degraded — it always gets priority bandwidth.
- **Encryption:** WebRTC mandates DTLS-SRTP. All media is encrypted end-to-end by default. This is a significant improvement over SPECS v1 which deferred encryption.
- **Auto-resume on transient network loss.** WebRTC's ICE restart mechanism handles network path changes (WiFi → cellular, IP change). The DataChannel heartbeat (1s interval) detects prolonged disconnection. If a peer is unreachable for 10 seconds, they are shown as disconnected. WebRTC will attempt ICE restart automatically.

### 4.3 Local Recording (The Golden Master)

This is the entire point of the product. Every participant records their own camera and microphone locally at full quality, independently of the live stream. This file is the deliverable.

#### 4.3.1 Local Video Recording

| Parameter | Value | Rationale |
|---|---|---|
| Resolution | Camera's native resolution, up to 1920x1080 (1080p) | getUserMedia requests 1080p; actual resolution documented in session metadata |
| Codec | H.264 (High Profile) via WebCodecs `VideoEncoder` | Universal editing compatibility; hardware-accelerated on most devices |
| Framerate | 30 fps constant | CFR is mandatory — editors need frame-accurate timelines |
| Bitrate | 8–15 Mbps VBR | Adjusted down from SPECS v1 (15–20 Mbps) to respect browser storage constraints while maintaining excellent quality |
| Keyframe interval | 1 second (GOP = 30 frames) | Makes editing / seeking fast without excessive file size |
| Color space | YUV 4:2:0 | Standard for H.264 delivery |
| Container | Fragmented MP4 via WASM muxer → OPFS | See §4.3.3 |

**Fallback chain:** If the browser doesn't support H.264 hardware encoding via WebCodecs, fall back to VP9 → VP8 (software). Quality may decrease, but recording never stops.

#### 4.3.2 Local Audio Recording

| Parameter | Value | Rationale |
|---|---|---|
| Codec | AAC-LC via WebCodecs `AudioEncoder`, or Opus fallback | AAC for NLE compatibility; Opus if AAC unavailable |
| Sample rate | 48 kHz | Web Audio API / WebCodecs native rate |
| Bitrate | 128–192 kbps CBR | Excellent quality for voice; AAC is more efficient than raw PCM |
| Channels | Mono (mic input is mono; stored as mono) | Honest representation of source |
| **Zero-drop guarantee** | Audio recording runs on a dedicated Audio Worklet with a ring buffer. Samples are buffered in SharedArrayBuffer and consumed by the WASM muxer. | This is non-negotiable. |

#### 4.3.3 Container, Storage & Crash Safety

- **Muxer:** A Rust-compiled WASM module ports the existing fragmented MP4 (fMP4) muxer from SPECS v1. This muxer runs in a dedicated Web Worker.
- **Storage:** Fragments are written to the **Origin Private File System (OPFS)** via the File System Access API's synchronous methods (available in Web Workers). OPFS provides high-performance, quota-managed local storage without user interaction.
- **Why OPFS:** Unlike `localStorage` or IndexedDB, OPFS supports synchronous, streaming writes from a Worker — essential for real-time muxing without back-pressure stalling the encoder.
- **Fragment interval:** Every 1 second, matching SPECS v1.
- **Crash safety:** If the browser tab crashes or power is lost, the OPFS file is recoverable up to the last flushed fragment. On session resume or next visit, the app detects the incomplete recording and offers recovery.
- **Finalization:** On clean session end, the WASM muxer writes a final `moov` atom, producing a standard-compatible MP4. The completed file is then offered for download via the browser's download mechanism, or saved directly via the File System Access API (with user permission).
- **Mobile considerations:** OPFS is supported on mobile browsers. Storage quotas vary by browser and device — the app checks available quota before recording and warns if less than 2 GB is available (approximately 15 minutes of recording at target bitrates).
- **Timestamp track:** Embed wall-clock UTC timestamps in metadata so that multi-participant recordings can be synchronized in post.

### 4.4 Synchronization Metadata

To make multi-cam editing possible, all participants must be time-aligned.

- **NTP-style time sync:** At session start, participants exchange timestamp messages over the reliable DataChannel. The WASM core runs the same simplified NTP algorithm from SPECS v1 (8 round-trip pings) to estimate clock offsets relative to the session creator's clock.
- **Embedded sync timecodes:** The local recording embeds session-relative timestamps so an editor can align all tracks by timecode alone.
- **Audio sync tone:** At session start, the app plays a short sync tone (1 kHz, 200ms) via Web Audio API that all participants record locally. This provides a sample-accurate sync point for editors, complementing the NTP-style timestamp sync.
- **`performance.now()` as timing source:** The WASM module uses `performance.now()` (microsecond resolution) for all internal timing. This is monotonic and unaffected by wall-clock adjustments.

### 4.5 UI

Clean, responsive, mobile-friendly. Built with standard web technologies (HTML/CSS/JS) orchestrated by a lightweight framework, with the WASM core handling all performance-critical logic.

- **Pre-call screen:**
  - Camera preview (local `<video>` element from getUserMedia).
  - Mic level meter (Web Audio API AnalyserNode).
  - Input device selection (MediaDevices.enumerateDevices).
  - "Create Room" or "Join Room" button.
  - Displays estimated recording file size and available OPFS quota.
  - Browser compatibility check: verifies WebCodecs, WebRTC, OPFS, and SharedArrayBuffer support. Displays clear guidance if any capability is missing.
- **Headphone prompt:** On entering the pre-call screen, a brief modal: "HyperZoom works best with headphones for clear audio. Please connect headphones before joining." Dismissible, but makes the expectation explicit. On mobile, this is even more important due to speaker proximity to microphone.
- **Create flow:** Click Create → Worker creates room → displays shareable link and room code → user shares via any messaging app.
- **Join flow:** Open shared link (or enter room code) → connect.
- **In-call screen:**
  - Responsive grid of participant video feeds (480p preview streams). Adapts layout for mobile (stacked) vs. desktop (2x2 grid).
  - Mic mute/unmute toggle.
  - Camera on/off toggle.
  - Recording indicator (always on — recording starts automatically when the call begins).
  - Network stats overlay (toggle): per-peer RTT, packet loss %, estimated audio latency.
  - "End Call" button.
  - Mobile: Full-screen mode with minimal chrome. Swipe gestures to switch between participant views on small screens.
- **Post-call screen:**
  - Download button for the local recording file.
  - Recording summary: duration, resolution, file size, codec info.
  - Option to save directly to filesystem (File System Access API, where supported).
  - Recovery notice if a previous incomplete recording was detected.

### 4.6 Permissions & Browser Security

- **COOP/COEP headers:** The Pages deployment must serve `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` to enable SharedArrayBuffer (required for lock-free ring buffers between Audio Worklet and WASM Worker).
- **getUserMedia permissions:** Camera and microphone access require user gesture and explicit permission grant. The UI must handle permission denied gracefully.
- **Secure context:** The app must be served over HTTPS (enforced by Cloudflare Pages).

---

## 5. Architecture

### 5.1 High-Level System Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CLOUDFLARE EDGE                                  │
│                                                                         │
│  ┌──────────────┐  ┌──────────────────┐  ┌──────────────────────────┐  │
│  │  CF Pages     │  │  CF Worker        │  │  CF Durable Object       │  │
│  │  (Static      │  │  (API + WS        │  │  (Per-Session State)     │  │
│  │   Client)     │  │   Upgrade)        │  │  - Participant list      │  │
│  │  HTML/CSS/JS  │  │  /api/rooms/*     │  │  - SDP relay             │  │
│  │  WASM bundle  │  │  /ws/signal       │  │  - ICE candidate relay   │  │
│  └──────────────┘  └────────┬─────────┘  │  - Session lifecycle      │  │
│                              │            └──────────────────────────┘  │
│  ┌──────────────┐            │                                          │
│  │  CF KV        │◄───────────┤  ┌──────────────┐  ┌────────────────┐  │
│  │  Room lookup  │            │  │  CF D1         │  │  CF R2          │  │
│  │  Invite codes │            │  │  Session       │  │  Recording      │  │
│  │  Tokens       │            │  │  history       │  │  backup         │  │
│  └──────────────┘            │  │  (post-MVP)    │  │  (post-MVP)     │  │
│                              │  └──────────────┘  └────────────────┘  │
│  ┌──────────────┐            │                                          │
│  │  CF TURN      │            │                                          │
│  │  Relay for    │◄──── NAT fallback                                    │
│  │  symmetric    │                                                      │
│  │  NATs         │                                                      │
│  └──────────────┘                                                      │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                    WebSocket (signaling)
                    WebRTC (media, P2P)
                              │
┌─────────────────────────────────────────────────────────────────────────┐
│                     BROWSER CLIENT (per participant)                     │
│                                                                         │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                        Main Thread                                │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────────┐ │  │
│  │  │  UI Layer    │  │  WebRTC      │  │  Media Devices           │ │  │
│  │  │  (HTML/CSS)  │  │  Manager     │  │  getUserMedia()          │ │  │
│  │  │              │  │  PeerConns   │  │  enumerateDevices()      │ │  │
│  │  └─────────────┘  │  DataChannels │  └──────────┬───────────────┘ │  │
│  │                    └──────┬───────┘             │                 │  │
│  └───────────────────────────┼─────────────────────┼─────────────────┘  │
│                              │                     │                     │
│  ┌───────────────────────────┼─────────────────────┼─────────────────┐  │
│  │                   Audio Worklet Thread          │                  │  │
│  │  ┌──────────────────────────────────────────┐   │                  │  │
│  │  │  AudioWorkletProcessor                    │   │                  │  │
│  │  │  - Captures raw PCM from mic stream       │   │                  │  │
│  │  │  - Writes to SharedArrayBuffer ring       │   │                  │  │
│  │  │  - Real-time priority (browser-managed)   │   │                  │  │
│  │  └──────────────────┬───────────────────────┘   │                  │  │
│  └─────────────────────┼───────────────────────────┼──────────────────┘  │
│                        │ SharedArrayBuffer          │                     │
│  ┌─────────────────────┼───────────────────────────┼──────────────────┐  │
│  │              Recording Worker (Web Worker + WASM)                   │  │
│  │                     │                           │                   │  │
│  │  ┌──────────────────▼─────┐  ┌──────────────────▼───────────────┐  │  │
│  │  │  Audio Encode (WASM)   │  │  Video Encode (WebCodecs)        │  │  │
│  │  │  AAC / Opus encoder    │  │  H.264 HW encoder                │  │  │
│  │  │  via WebCodecs         │  │  1080p @ 30fps                   │  │  │
│  │  └──────────┬─────────────┘  └──────────┬───────────────────────┘  │  │
│  │             │                            │                          │  │
│  │  ┌──────────▼────────────────────────────▼───────────────────────┐  │  │
│  │  │  fMP4 Muxer (WASM — ported from Rust)                         │  │  │
│  │  │  - Interleaves audio + video atoms                            │  │  │
│  │  │  - Flushes fragments every 1 second                           │  │  │
│  │  │  - Writes to OPFS (synchronous API in Worker)                 │  │  │
│  │  └───────────────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │                          OPFS (Origin Private File System)         │  │
│  │  /recordings/{session-id}/                                         │  │
│  │    ├── local_recording.mp4      (fMP4 fragments, crash-safe)       │  │
│  │    └── session_metadata.json    (session info + sync data)         │  │
│  └────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 5.2 Client Threading Model (Browser)

Browsers do not expose OS threads. Instead, we use the browser's concurrency primitives — Web Workers, Audio Worklets, and the main thread — to approximate the threading model from SPECS v1.

| Context | Browser Primitive | Priority | Responsibility | Notes |
|---|---|---|---|---|
| **Audio Capture** | AudioWorkletProcessor | Real-time (browser-managed) | Pulls raw PCM samples from mic MediaStream, writes to SharedArrayBuffer ring | Runs on the audio rendering thread; ~3ms callback interval at 48kHz/128 samples. Must never block. |
| **Audio Playback** | AudioWorkletProcessor | Real-time (browser-managed) | Reads decoded audio from remote peers, mixes, outputs to speakers | Jitter buffer consumed here. WebRTC handles primary jitter; worklet handles local playback timing. |
| **Live Audio/Video** | WebRTC (browser-internal) | High (browser-managed) | Opus encode/decode, VP8/VP9 encode/decode, ICE, DTLS | Entirely managed by the browser's WebRTC stack. We configure via SDP constraints but don't touch raw packets. |
| **Recording Worker** | Web Worker + WASM | Normal | Reads raw audio from SharedArrayBuffer ring, encodes via WebCodecs (AAC/H.264), muxes to fMP4, writes to OPFS | Dedicated worker ensures muxing never blocks UI or audio. |
| **Video Capture** | Main thread (getUserMedia) | Normal | Provides MediaStream to WebRTC tracks and to Recording Worker via `VideoFrame` transfer | Frames are transferred (zero-copy) to the Recording Worker via `postMessage` with `transfer`. |
| **UI / Render** | Main thread | Normal | DOM updates, user interaction, WebRTC peer connection management, signaling WebSocket | Standard browser event loop. |

### 5.3 Shared Memory Architecture

The Audio Worklet and Recording Worker communicate via **SharedArrayBuffer** to avoid the latency of `postMessage` for real-time audio data:

```
AudioWorkletProcessor                    Recording Worker (WASM)
      │                                          │
      │  ┌──────────────────────────────────┐    │
      ├──►  SharedArrayBuffer Ring (PCM)    ├────┤
      │  │  - 48kHz mono f32 samples        │    │
      │  │  - ~500ms capacity (~24,000      │    │
      │  │    samples)                       │    │
      │  │  - SPSC: Worklet writes,         │    │
      │  │    Worker reads                   │    │
      │  │  - Atomic read/write pointers     │    │
      │  └──────────────────────────────────┘    │
      │                                          │
```

Video frames follow a different path: `MediaStreamTrackProcessor` on the main thread (or a Worker) reads `VideoFrame` objects from the camera stream. These are transferred to the Recording Worker via `postMessage` with `Transferable` semantics (zero-copy).

### 5.4 Signaling Protocol (WebSocket)

All signaling flows through the Cloudflare Worker → Durable Object via WebSocket. Messages are JSON-encoded for simplicity (signaling is infrequent; performance is not critical).

#### Message Types

```typescript
// Client → Server
{ type: "create_room", display_name: string }
{ type: "join_room", room_code: string, display_name: string }
{ type: "sdp_offer", target_peer_id: number, sdp: string }
{ type: "sdp_answer", target_peer_id: number, sdp: string }
{ type: "ice_candidate", target_peer_id: number, candidate: string }
{ type: "leave" }

// Server → Client
{ type: "room_created", room_code: string, peer_id: number }
{ type: "room_joined", peer_id: number, peers: PeerInfo[] }
{ type: "peer_joined", peer: PeerInfo }
{ type: "peer_left", peer_id: number }
{ type: "sdp_offer", from_peer_id: number, sdp: string }
{ type: "sdp_answer", from_peer_id: number, sdp: string }
{ type: "ice_candidate", from_peer_id: number, candidate: string }
{ type: "error", code: string, message: string }
```

#### Connection Sequence

1. **Creator → Worker:** `create_room` → Worker creates Durable Object, generates room code, stores in KV → `room_created` response.
2. **Joiner → Worker:** `join_room` with room code → Worker looks up Durable Object in KV → DO assigns participant ID → `room_joined` response with existing peer list.
3. **DO → All existing peers:** `peer_joined` notification.
4. **New peer → Each existing peer:** `sdp_offer` (WebRTC offer via signaling relay).
5. **Each existing peer → New peer:** `sdp_answer` (WebRTC answer via signaling relay).
6. **Bidirectional ICE candidate exchange** until peer connections are established.
7. **DataChannel opens:** Participants exchange time-sync pings (8 round trips).
8. **Media streams begin.**

### 5.5 Data Channel Protocol

Once WebRTC peer connections are established, a reliable ordered DataChannel named `"control"` is used for application-level control messages. These replace the custom UDP control packets from SPECS v1.

Messages are binary-encoded (not JSON) for efficiency, using a simple tag-length-value format:

| Message | Tag | Payload | Purpose |
|---|---|---|---|
| Heartbeat | `0x01` | `u32` timestamp_ms | Liveness detection (1s interval) |
| TimeSync Request | `0x02` | `u64` sender_time_us | NTP-style clock offset estimation |
| TimeSync Response | `0x03` | `u64` sender_time_us, `u64` responder_time_us | Clock offset calculation |
| Recording Started | `0x04` | `u64` start_timestamp_us | Notify peers that local recording is active |
| Recording Stopped | `0x05` | `u64` stop_timestamp_us, `u32` total_frames | Notify peers that recording ended |
| Stats Report | `0x06` | Variable (RTT, loss, jitter) | Periodic quality metrics exchange |
| Bye | `0x07` | (none) | Graceful disconnect |

#### Heartbeat & Disconnect

- **Heartbeat interval:** Every participant sends a heartbeat via DataChannel to each peer every **1 second**.
- **Timeout:** If no DataChannel messages (heartbeat or other) are received from a peer for **10 seconds**, that peer is considered disconnected. The 10-second window (vs. 5s in SPECS v1) accounts for WebRTC's ICE restart attempts on network path changes.
- **Graceful disconnect:** When a participant ends the call, they send a `Bye` message on all DataChannels and a `leave` message on the signaling WebSocket. Peers immediately remove the participant.

### 5.6 Bandwidth Budget (per peer pair, per direction)

| Stream | Bitrate | Notes |
|---|---|---|
| Audio (Opus via WebRTC) | 32 kbps | Configured via SDP maxaveragebitrate |
| Video (VP8/VP9 480p via WebRTC) | 300–500 kbps | Configured via setParameters() maxBitrate |
| DataChannel overhead | ~2 kbps | Heartbeats, sync, stats |
| DTLS/SRTP overhead | ~5% | Encryption overhead vs. plain UDP |
| **Total** | **~370–570 kbps** | Comparable to SPECS v1 with encryption included |

With 3 outgoing streams (full mesh, 4 participants), worst-case upload is ~1.7 Mbps — well within typical broadband and most cellular connections.

---

## 6. Server Architecture (Cloudflare)

### 6.1 Cloudflare Worker (Entry Point)

The Worker handles HTTP requests and WebSocket upgrades:

```
Routes:
  GET  /                          → Serve Pages (static client)
  GET  /api/rooms/:code/exists    → Check if room exists (KV lookup)
  GET  /ws/signal?room=:code      → WebSocket upgrade → route to Durable Object
  POST /api/rooms                 → Create room → create Durable Object, store in KV
```

The Worker is stateless. All session state lives in the Durable Object.

### 6.2 Durable Object (Per-Session State)

Each active session has one Durable Object instance managing:

- **Participant list:** ID, display name, WebSocket connection handle, join time.
- **Signaling relay:** Forwards SDP offers/answers and ICE candidates between peers.
- **Session lifecycle:** Tracks creation time, participant joins/leaves, session end.
- **Auto-cleanup:** If all participants disconnect, the DO closes after a 5-minute grace period (in case of brief disconnection). It persists session metadata to D1 (or KV) before shutting down.

**Scaling:** Durable Objects run on exactly one Cloudflare edge node, providing single-threaded consistency for session state. Since each session has at most 4 participants and signaling is lightweight, a single DO can handle the load easily.

### 6.3 Cloudflare KV (Room Lookup)

- **Key:** Room code (e.g., `hype-blue-fox-42`)
- **Value:** Durable Object ID
- **TTL:** 24 hours (rooms expire if unused)

Room codes are generated by the Worker using a word-list format for human readability: `{adjective}-{color}-{animal}-{number}`.

### 6.4 Cloudflare TURN (NAT Relay)

For participants behind symmetric NATs where direct P2P connection fails, Cloudflare's TURN service provides relay:

- The Worker generates short-lived TURN credentials (valid for the session duration).
- Credentials are included in the `room_joined` / `room_created` signaling response.
- The client includes these TURN servers in its `RTCPeerConnection` ICE configuration.
- WebRTC automatically falls back to TURN when direct or STUN-assisted connections fail.

### 6.5 Cloudflare D1 (Persistent Storage — Post-MVP)

Schema for session history:

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  room_code TEXT NOT NULL,
  created_at TEXT NOT NULL,
  ended_at TEXT,
  duration_seconds REAL,
  participant_count INTEGER
);

CREATE TABLE participants (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  peer_id INTEGER NOT NULL,
  display_name TEXT NOT NULL,
  joined_at TEXT NOT NULL,
  left_at TEXT,
  recording_resolution TEXT,
  recording_codec TEXT
);
```

### 6.6 Cloudflare R2 (Recording Backup — Post-MVP)

Optional cloud backup for recordings. After session end, the client can upload the finalized MP4 to R2 via a pre-signed URL generated by the Worker. This enables:
- Cross-device access to recordings.
- Sharing recordings with the editor via a download link.
- Backup in case of local storage loss.

---

## 7. Technology Stack — Client

### 7.1 WASM Core (Rust → wasm32)

| Concern | Implementation | Notes |
|---|---|---|
| WASM bindings | `wasm-bindgen` + `web-sys` + `js-sys` | Rust ↔ JavaScript interop |
| fMP4 muxer | Port of existing `src/audio/fmp4.rs` | Core recording pipeline — runs in Web Worker |
| Ring buffers | `ringbuf` or custom SPSC on SharedArrayBuffer | Shared between Audio Worklet and Recording Worker |
| Time sync | Port of existing NTP algorithm | Runs in main thread WASM, uses DataChannel |
| Session metadata | `serde` + `serde_json` (wasm-compatible) | Generate session_metadata.json |
| Protocol encoding | Custom binary encoder | DataChannel control messages |
| Build toolchain | `wasm-pack` or `cargo` + `wasm-bindgen-cli` | Targets `wasm32-unknown-unknown` |

### 7.2 Browser APIs (via JavaScript / web-sys)

| Concern | Browser API | Notes |
|---|---|---|
| Camera/mic capture | `navigator.mediaDevices.getUserMedia()` | Replaces `cpal` + `nokhwa` from SPECS v1 |
| Device enumeration | `navigator.mediaDevices.enumerateDevices()` | Replaces `cpal` device listing |
| Video encoding (local rec) | `VideoEncoder` (WebCodecs API) | H.264 HW-accelerated; replaces VideoToolbox/MF |
| Audio encoding (local rec) | `AudioEncoder` (WebCodecs API) | AAC or Opus; replaces `fdk-aac` |
| Video decoding | Handled by WebRTC | Browser decodes incoming VP8/VP9 streams |
| Audio codec (live) | Handled by WebRTC (Opus) | Browser's built-in Opus; replaces `opus` crate |
| P2P transport | `RTCPeerConnection` | Replaces raw UDP + custom protocol |
| Data messaging | `RTCDataChannel` | Replaces custom UDP control packets |
| Signaling transport | `WebSocket` | Connects to Cloudflare Worker |
| Audio processing | `AudioWorklet` + `AudioContext` | Replaces `cpal` for capture/playback |
| Local file storage | Origin Private File System (OPFS) | Replaces direct filesystem I/O |
| File download | `File System Access API` / Blob download | User-facing file export |
| Threading | `Web Worker` + `SharedArrayBuffer` | Replaces `std::thread` |
| High-res timing | `performance.now()` | Replaces `std::time::Instant` |
| Wakelock | `navigator.wakeLock` | Prevents screen sleep during recording (mobile) |

### 7.3 Web UI

| Concern | Technology | Notes |
|---|---|---|
| Framework | Vanilla JS or lightweight (Preact/Solid) | Minimize bundle size; UI is simple |
| Styling | CSS with responsive breakpoints | Mobile-first design |
| Layout | CSS Grid for video tiles | Adaptive: 2x2 desktop, stacked mobile |
| Icons | Inline SVG | No icon font dependency |
| Build | Vite or esbuild | Fast builds, WASM integration |

### 7.4 Tauri Desktop Shell (Secondary)

| Concern | Technology | Notes |
|---|---|---|
| Native wrapper | Tauri v2 | Wraps the web client in native WebView |
| macOS | WKWebView | Apple Silicon + Intel via universal binary |
| Windows | WebView2 (Chromium) | x86_64; full WebCodecs support |
| File access | Tauri FS API | Direct filesystem for recordings (bypass OPFS) |
| Tray/notifications | Tauri plugins | Native OS integration |

---

## 8. Local Recording File Layout

### 8.1 Browser (OPFS)

```
opfs:/recordings/
  └── {session-id}/
      ├── local_recording.mp4          # fMP4 fragments (crash-recoverable)
      └── session_metadata.json         # Session info, participants, clock offsets
```

After session end, the finalized MP4 is offered for download. The user saves it wherever they like.

### 8.2 Tauri Desktop

```
~/HyperZoom/recordings/
  └── 2026-03-02_20-15-00_{room-code}/
      ├── local_recording.mp4          # Same fMP4 → finalized MP4
      └── session_metadata.json         # Same metadata format
```

### 8.3 session_metadata.json

```json
{
  "session_id": "a1b2c3d4",
  "room_code": "hype-blue-fox-42",
  "started_at_utc": "2026-03-02T20:15:00.000Z",
  "duration_seconds": 3842,
  "participants": [
    {
      "id": 0,
      "name": "Alice (Creator)",
      "clock_offset_ms": 0
    },
    {
      "id": 1,
      "name": "Bob",
      "clock_offset_ms": -12
    }
  ],
  "recording": {
    "video_codec": "H.264 High",
    "video_resolution": "1920x1080",
    "video_fps": 30,
    "video_bitrate_kbps": 12000,
    "audio_codec": "AAC-LC",
    "audio_sample_rate": 48000,
    "audio_channels": 1,
    "audio_bitrate_kbps": 192,
    "container": "fMP4",
    "total_frames_captured": 115260,
    "total_frames_dropped": 0,
    "platform": "browser",
    "browser": "Chrome 122",
    "webcodecs_hw_accelerated": true
  }
}
```

---

## 9. Quality Assurance & Invariants

These are **hard invariants** that must hold in every build:

1. **Local recording never drops frames.** The Recording Worker must consume VideoFrames fast enough. If the WebCodecs encoder falls behind, the ring buffer / frame queue must absorb the delay. If frames are dropped, the session metadata logs the count — but the system must be engineered to prevent this.
2. **Local recording never drops audio samples.** The Audio Worklet → SharedArrayBuffer → WASM muxer pipeline must be sized to absorb scheduling jitter. The SharedArrayBuffer ring holds ~500ms of audio as a safety cushion.
3. **Audio has priority over video in WebRTC.** We configure WebRTC transceiver priorities: audio `high`, video `low`. The browser's congestion controller respects these priorities.
4. **Live video degrades before audio.** Under congestion: reduce video bitrate → reduce framerate → reduce resolution. Audio configuration is never touched.
5. **Session end produces a valid MP4.** The finalization step (writing final `moov` atom) must complete before offering download. If the user closes the tab during finalization, the fMP4 on OPFS is still playable by ffmpeg/VLC.
6. **Signaling server cleanup.** Durable Objects self-destruct after all participants leave (with a 5-minute grace period). KV entries expire via TTL. No orphaned state.
7. **Graceful degradation.** If WebCodecs H.264 is unavailable, fall back to VP9 → VP8. If OPFS is unavailable (very old browser), warn the user and disable recording. The live call still works.
8. **Secure by default.** All media is DTLS-SRTP encrypted (WebRTC mandate). All signaling is WSS (TLS). No unencrypted data in transit.

---

## 10. Performance Targets

| Metric | Browser Target | Tauri Desktop Target | Measurement Method |
|---|---|---|---|
| Audio one-way latency (mouth-to-ear) | < 100 ms (internet) | < 80 ms (internet) | Loopback measurement via DataChannel timestamps |
| Audio worklet processing latency | < 5 ms per callback | N/A (same WebView) | `performance.now()` instrumentation |
| Video capture-to-display latency | < 200 ms | < 150 ms | Timestamp comparison |
| Local recording frame drop rate | 0% (target) | 0% (target) | session_metadata.json frame count |
| WASM fMP4 muxer throughput | > 60 fps sustained | > 60 fps sustained | Benchmark in Worker |
| OPFS write latency (per fragment) | < 50 ms | N/A (direct FS) | `performance.now()` instrumentation |
| Initial page load (cold) | < 3 seconds (WASM + UI) | < 2 seconds | Lighthouse / stopwatch |
| WASM bundle size (gzipped) | < 500 KB | N/A | Build output |
| Total client bundle (gzipped) | < 1 MB | N/A | Build output |
| Memory usage | < 200 MB | < 250 MB | Browser DevTools |
| Mobile battery drain | < 15% per hour | N/A | Device monitoring |

---

## 11. MVP Scope & Non-Goals

### In Scope (MVP)

- [ ] Browser WASM client with WebRTC full-mesh connectivity (up to 4 participants)
- [ ] Cloudflare Worker + Durable Object signaling server
- [ ] Room creation with shareable link / room code
- [ ] WebRTC with STUN + TURN for universal NAT traversal
- [ ] Low-latency Opus audio streaming (WebRTC)
- [ ] 480p VP8/VP9 video preview streaming (WebRTC)
- [ ] Local 1080p H.264 recording via WebCodecs
- [ ] Local AAC audio recording via WebCodecs (Opus fallback)
- [ ] WASM fMP4 muxer with OPFS crash-safe storage
- [ ] Recording download on session end
- [ ] NTP-style clock sync via DataChannel for multi-cam timecode alignment
- [ ] Audio sync tone at session start
- [ ] Responsive web UI (desktop + mobile)
- [ ] Headphone prompt
- [ ] OPFS quota check and recording size estimate
- [ ] Mic mute / camera toggle
- [ ] Auto-start recording on call join
- [ ] Heartbeat and graceful disconnect via DataChannel
- [ ] Browser compatibility check on load
- [ ] COOP/COEP headers for SharedArrayBuffer
- [ ] Secure by default (DTLS-SRTP, WSS)

### Post-MVP

- [ ] Tauri desktop builds (macOS Apple Silicon, macOS Intel, Windows x86_64)
- [ ] Cloudflare D1 for session history and user accounts
- [ ] Cloudflare R2 cloud backup for recordings
- [ ] FLAC / lossless audio recording option
- [ ] Screen sharing (WebRTC screen capture)
- [ ] Text chat (DataChannel)
- [ ] Recording recovery UI (detect and recover incomplete OPFS recordings)
- [ ] Echo cancellation (Web Audio API + browser AEC)
- [ ] Noise suppression (Web Audio API)
- [ ] Virtual backgrounds (WebGL / ML-based segmentation)
- [ ] Custom resolution / bitrate settings UI
- [ ] Pre-signed share links for recordings (R2 + Worker)
- [ ] PWA mode (installable, offline shell)
- [ ] Push notifications for session invites
- [ ] Multi-monitor / PiP (Picture-in-Picture API)
- [ ] End-to-end encryption beyond DTLS-SRTP (Insertable Streams API)

---

## 12. Post-Session Workflow (User Story)

1. Alice opens `hyperzoom.app` in her browser and clicks "Create Room." The app generates a link: `hyperzoom.app/hype-blue-fox-42`.
2. Alice shares the link in the group chat. Bob, Carol, and Dave click it on their devices — Bob on his laptop Chrome, Carol on her iPhone Safari, Dave on his Android Chrome.
3. Everyone grants camera and microphone permissions. The app checks browser compatibility (all green) and shows the pre-call screen with camera preview and mic level meter.
4. They join the room. WebRTC connections are established in seconds. The conversation feels natural — audio latency is low enough that nobody talks over each other. The 480p video preview is modest but good enough to see faces and reactions.
5. Behind the scenes, each participant's browser is locally encoding 1080p H.264 video and AAC audio via WebCodecs, muxing to fragmented MP4 in a Web Worker, and streaming fragments to OPFS.
6. After 60 minutes, everyone clicks "End Call." Each participant sees a download button. They download their `local_recording.mp4` — pristine 1080p, crystal-clear audio, zero dropped frames.
7. Files are shared via Google Drive / Dropbox / WeTransfer. The editor imports all four MP4s into DaVinci Resolve. The embedded timecodes snap all tracks into alignment.
8. The final podcast looks and sounds professional because every frame came from a local, hardware-accelerated recording — not a compressed stream.

---

## 13. Migration Notes from SPECS v1

| SPECS v1 (Native) | SPECS v2 (Browser-First) | Rationale |
|---|---|---|
| Raw UDP with custom RTP-like framing | WebRTC (PeerConnection + DataChannel) | Browsers cannot send raw UDP. WebRTC provides equivalent P2P media transport with mandatory encryption. |
| UPnP for NAT traversal | ICE + STUN + TURN (Cloudflare) | UPnP is unavailable in browsers. ICE is WebRTC's standard NAT traversal and works with all NAT types. |
| Manual IP:port exchange | Room codes + shareable links | Browser users expect links, not IP addresses. |
| `cpal` for audio I/O | Web Audio API (AudioWorklet) | Browser audio capture/playback API. Slightly higher latency (~10–20ms) but universally available. |
| `nokhwa` for camera capture | `getUserMedia()` | Browser camera API. Handles permissions, device selection, resolution negotiation. |
| `opus` crate (C FFI) | WebRTC built-in Opus | Browser's native Opus implementation. Cannot configure 2.5/5ms frames (10ms minimum). |
| `vpx` crate (C FFI) | WebRTC built-in VP8/VP9 | Browser's native implementation with hardware acceleration. |
| `fdk-aac` (C FFI) | WebCodecs `AudioEncoder` | Browser-native AAC encoding (hardware-accelerated). |
| VideoToolbox / Media Foundation (C FFI) | WebCodecs `VideoEncoder` | Browser-native H.264 hardware encoding. Cross-platform without C FFI. |
| `std::thread` + OS RT priority | Web Workers + Audio Worklets | No OS thread access in browsers. Audio Worklet runs on browser's RT audio thread. |
| SPSC ring buffers (`ringbuf`) | SharedArrayBuffer + Atomics | Same SPSC pattern, implemented on shared memory visible to both Worklet and Worker. |
| Direct filesystem I/O | OPFS (Origin Private File System) | Browser sandboxed filesystem. High-performance synchronous access in Workers. |
| `egui` / `eframe` | HTML/CSS/JS (responsive web UI) | Native web UI is better for mobile, accessibility, and responsive design. |
| Single Rust binary | WASM bundle + web assets on Cloudflare Pages | Hosted, no install, instant updates. |
| No server | Cloudflare Workers + Durable Objects | Signaling server required for WebRTC. Serverless = zero ops. |
| No encryption (MVP) | DTLS-SRTP (mandatory in WebRTC) | Security by default. No opt-in needed. |
| 5ms Opus frames | 10ms Opus frames (browser minimum) | Browser WebRTC implementation constraint. +5ms latency contribution. |
| Sub-50ms audio latency target | Sub-100ms audio latency target | Browser overhead is real. Still excellent for conversation. |

---

## 14. Open Questions

- **WebCodecs H.264 on iOS Safari:** As of Safari 16.4, WebCodecs is available but H.264 encoding support varies by device. Need to test on older iPhones (A12 and earlier). Fallback to VP9/VP8 may be necessary.
- **OPFS quota on mobile:** Mobile browsers have varying storage quotas (often 50–100MB by default, up to 20% of disk with persisted storage permission). A 60-minute 1080p recording at 12 Mbps is ~5.4 GB. We may need to prompt for persistent storage permission or reduce bitrate on mobile.
- **SharedArrayBuffer + COOP/COEP:** These headers break some third-party embeds and cross-origin resources. Ensure all assets (fonts, analytics, etc.) are same-origin or have appropriate CORS headers.
- **Cloudflare TURN pricing:** Cloudflare's TURN relay has usage-based pricing. For symmetric NAT users, media relay costs could be significant. Evaluate pricing for 4-person sessions with 1+ hour duration.
- **Echo cancellation:** MVP assumes headphones. Browsers have built-in AEC via `getUserMedia({ audio: { echoCancellation: true } })`, but quality varies. For post-MVP, evaluate whether browser AEC is sufficient or if additional processing is needed.
- **Opus vs. AAC for local recording:** AAC has better NLE compatibility but Opus has broader WebCodecs support. Default to AAC where available, Opus as fallback. Monitor browser support evolution.
- **Tauri WebView WebCodecs support:** WKWebView (macOS) may lag behind Safari in WebCodecs support. Need to verify feature availability in Tauri's embedded WebView for the desktop build.
- **Recording during backgrounded tab (mobile):** Mobile browsers aggressively throttle or suspend background tabs. If the user switches apps during a call, recording may pause. Investigate `visibilitychange` event handling and warn users about backgrounding. Picture-in-Picture API may help keep the tab active.
