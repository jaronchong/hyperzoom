# HyperZoom — Implementation Plan

> **Derived from:** [SPECS.md](./SPECS.md) v0.1 MVP
> **Last updated:** 2026-02-28

Each stage ends with a **compileable, runnable binary** and a concrete milestone the user can validate. Tasks are numbered `Stage.Group.Subtask`. Stages build on each other sequentially.

---

## Stage 1: Project Bootstrap & Audio Pipeline

**Goal:** A Rust application that opens a window, captures microphone audio, and plays it back through speakers in a low-latency loopback. This proves the real-time audio foundation before any networking is added.

### 1.1 Project Setup

- [ ] **1.1.1** Initialize Cargo workspace with `Cargo.toml`. Add initial dependencies: `eframe`/`egui`, `cpal`, `ringbuf`, `tokio`.
- [ ] **1.1.2** Set up conditional compilation for macOS and Windows platform-specific code (`#[cfg(target_os)]` modules).
- [ ] **1.1.3** Create `main.rs` entry point: initialize tokio runtime, launch eframe/egui window with a blank "HyperZoom" title bar.

### 1.2 Audio Device Layer

- [ ] **1.2.1** Integrate `cpal`: enumerate available audio input devices (microphones) and output devices (speakers/headphones). Print device list to console on startup.
- [ ] **1.2.2** Open audio input stream — 48 kHz, mono, f32 samples — using the system default input device.
- [ ] **1.2.3** Open audio output stream — 48 kHz, mono, f32 samples — using the system default output device.
- [ ] **1.2.4** Implement a SPSC (single-producer, single-consumer) lock-free ring buffer for audio using the `ringbuf` crate. Size it for ~200 ms of audio at 48 kHz mono (~9,600 samples).
- [ ] **1.2.5** Wire the loopback path: audio capture callback pushes samples into the ring buffer → audio playback callback pulls samples from the ring buffer → you hear your own mic through your speakers.

### 1.3 Real-Time Thread Scheduling

- [ ] **1.3.1** macOS: implement a helper function that elevates the calling thread to real-time priority using `thread_policy_set` with `THREAD_TIME_CONSTRAINT_POLICY`.
- [ ] **1.3.2** Windows: implement a helper function that elevates the calling thread to real-time priority using MMCSS (`AvSetMmThreadCharacteristicsW` with "Pro Audio" task).
- [ ] **1.3.3** Apply real-time priority to the audio capture thread and audio playback thread at startup. Log a warning if elevation fails (e.g., insufficient privileges).

### Milestone 1

> **Launch the app. An egui window appears. You hear your own microphone played back through your speakers/headphones with very low latency. Console logs show the selected audio devices and thread priority status.**

---

## Stage 2: Network Protocol & Live Audio Streaming

**Goal:** Two instances of HyperZoom on separate machines (or separate ports on localhost) connect over UDP. You hear the other person's voice in real-time. This proves the entire latency-critical audio path end-to-end.

### 2.1 Packet Protocol

- [ ] **2.1.1** Define the 12-byte packet header as a Rust struct per §5.4: version (2 bits), padding (1 bit), type (5 bits), participant ID (8 bits), sequence number (16 bits), timestamp in milliseconds (32 bits), payload length (16 bits), fragment ID (8 bits), fragment total (8 bits).
- [ ] **2.1.2** Implement `to_bytes()` and `from_bytes()` serialization for the packet header. Use big-endian byte order.
- [ ] **2.1.3** Define Rust enums/structs for control packet payload types: `Hello` (display name, version), `Welcome` (session ID, assigned participant ID, peer list), `PeerJoined`, `Heartbeat`, `Nack`, `Bye`.
- [ ] **2.1.4** Implement serialization/deserialization for each control payload type. Use a compact binary format (no JSON on the wire).

### 2.2 UDP Networking

- [ ] **2.2.1** Create a UDP socket manager using `tokio::net::UdpSocket`. Bind to a configurable port. Support sending to and receiving from multiple peer addresses.
- [ ] **2.2.2** Implement a packet receive loop (tokio task) that reads incoming UDP packets, parses the 12-byte header, and dispatches by packet type (audio → audio pipeline, control → control handler, video → future video pipeline).
- [ ] **2.2.3** Integrate the `igd` crate: on session start, discover the local UPnP gateway and request a UDP port mapping. Store the external IP and mapped port.
- [ ] **2.2.4** Implement UPnP port unmapping: on clean shutdown, remove the port mapping from the router.

### 2.3 Session Handshake

- [ ] **2.3.1** Implement the host-side handshake: listen for incoming `Hello` packets → assign a participant ID → respond with `Welcome` packet containing session ID, participant ID, and list of existing peers with their IP:port.
- [ ] **2.3.2** Implement the guest-side handshake: send `Hello` to host's IP:port → receive `Welcome` → store session info and peer list.
- [ ] **2.3.3** Implement a per-peer connection state machine with states: `Connecting`, `Connected`, `Disconnected`. Track state transitions and expose them to the UI.

### 2.4 Opus Audio Codec

- [ ] **2.4.1** Integrate the `opus` crate. Create an Opus encoder configured for: 48 kHz, mono, 5 ms frame size (240 samples/frame), 32 kbps CBR, low-delay application mode.
- [ ] **2.4.2** Create an Opus decoder for incoming audio. Configure for 48 kHz, mono.
- [ ] **2.4.3** Enable Opus in-band FEC (Forward Error Correction) and DTX (Discontinuous Transmission — suppresses packets during silence) on the encoder.
- [ ] **2.4.4** Build the outbound audio pipeline on a dedicated raw OS thread (not tokio): audio capture ring → accumulate 240 samples (5 ms) → Opus encode → wrap in packet with header → UDP send. This thread runs at real-time priority.

### 2.5 Audio Receive & Playback

- [ ] **2.5.1** Implement an adaptive jitter buffer: holds incoming decoded audio packets ordered by timestamp. Starts with minimal delay (~5 ms) and grows up to 30 ms if jitter/loss is detected. Shrinks back when conditions improve.
- [ ] **2.5.2** Build the inbound audio pipeline: UDP recv (tokio) → demux audio packets → Opus decode → push into jitter buffer → audio playback thread pulls from jitter buffer and writes to OS audio output.
- [ ] **2.5.3** Implement Opus PLC (Packet Loss Concealment): when the jitter buffer has a gap (missing sequence number), call the Opus decoder with the "lost packet" flag to generate a concealment frame instead of silence.

### 2.6 Heartbeat & Disconnect

- [ ] **2.6.1** Implement heartbeat sending: every 1 second, send a `Heartbeat` control packet to each connected peer (from a tokio timer task).
- [ ] **2.6.2** Implement peer timeout detection: track the timestamp of the last received packet (any type) from each peer. If >5 seconds elapse with no packets, mark the peer as `Disconnected`.
- [ ] **2.6.3** Implement BYE sending: on End Call, send a `Bye` control packet to all peers 3 times at 50 ms intervals for reliability.
- [ ] **2.6.4** Handle BYE reception: when a `Bye` packet is received, immediately mark that peer as `Disconnected` and stop rendering their streams.

### Milestone 2

> **Run two instances (on two machines or two terminal windows using different ports on localhost). One hosts, one joins by entering the host's IP:port. Both participants hear each other's voice in real-time. Pressing End Call sends BYE and the other instance detects the disconnect. Audio latency on LAN should be noticeably low (<50 ms).**

---

## Stage 3: Local Audio Recording

**Goal:** While the live Opus stream flows between peers, each participant simultaneously records their own microphone to a local AAC + fragmented MP4 file. This introduces the dual-pipeline architecture (live vs. local) and the crash-safe container.

### 3.1 Dual Audio Ring Buffers

- [ ] **3.1.1** Modify the audio capture thread to write into **two** independent SPSC ring buffers instead of one: Ring A feeds the Opus live encoder (already built), Ring B feeds the new AAC local recorder. Each ring is sized for ~200 ms.
- [ ] **3.1.2** Verify that both consumers (live encoder + local recorder) receive every sample independently with no drops, even when one consumer is temporarily slow.

### 3.2 AAC Encoding

- [ ] **3.2.1** Integrate `fdk-aac` crate (or platform-native AAC encoder). Configure for AAC-LC, 48 kHz, mono, 192 kbps CBR.
- [ ] **3.2.2** Create a dedicated raw OS thread for AAC encoding: reads PCM samples from Ring B → encodes to AAC frames → pushes encoded frames to the MP4 muxer. Runs at high (but not real-time) priority.

### 3.3 Fragmented MP4 Muxer (Audio-Only)

- [ ] **3.3.1** Implement (or integrate) a fragmented MP4 (fMP4) writer that produces valid `ftyp`, `moov` (with track descriptions), then periodic `moof`+`mdat` atom pairs containing AAC frames. This runs as a tokio task.
- [ ] **3.3.2** Configure the muxer to flush a new fragment to disk every 1 second. Use buffered async file I/O (tokio::fs or std::io::BufWriter) to avoid blocking.
- [ ] **3.3.3** Implement MP4 finalization: on clean session end, write a final compatible `moov` atom so the file is a fully standard MP4 playable everywhere (not just fMP4-aware players).
- [ ] **3.3.4** Verify crash recovery: forcefully kill the process mid-recording. Confirm the partial fMP4 file is playable (up to the last flushed fragment) in VLC or ffmpeg.

### 3.4 Session File Management

- [ ] **3.4.1** On session start, create the session directory: `~/HyperZoom/recordings/YYYY-MM-DD_HH-MM-SS/`. Write the recording file as `local_recording.mp4` inside this directory.
- [ ] **3.4.2** On session end, write `session_metadata.json` with: session ID, UTC start time, duration, participant list, and recording stats (codec, sample rate, bitrate, container format).
- [ ] **3.4.3** Wire recording lifecycle: auto-start recording when the call connects, auto-stop and finalize when End Call is pressed.

### Milestone 3

> **Have a two-person call, talk for 30+ seconds, end the call. Each participant finds `~/HyperZoom/recordings/<timestamp>/local_recording.mp4` on their machine. Open it in VLC — you hear your own mic audio in high-quality AAC. The `session_metadata.json` file is present and correctly populated.**

---

## Stage 4: Video Capture & Live Preview Streaming

**Goal:** Add camera capture and 480p VP8 video streaming to the existing audio-only call. Both participants see each other's faces in a grid alongside hearing each other. Video is lower priority than audio at every level.

### 4.1 Camera Capture

- [ ] **4.1.1** Integrate `nokhwa` (or platform-native camera FFI). Enumerate available video devices and print to console.
- [ ] **4.1.2** Open the selected camera at its native resolution (up to 1080p) at 30 fps. Receive raw frames (RGB or YUV).
- [ ] **4.1.3** Create a SPSC ring buffer for video frames (sized for 4–6 frames). The camera capture callback pushes raw frames into this ring.
- [ ] **4.1.4** Display the local camera preview in the egui window (render the latest frame from the ring buffer as a texture).

### 4.2 VP8 Live Encode

- [ ] **4.2.1** Integrate the `vpx` crate. Configure a VP8 encoder: 854x480 output, 24 fps, 300–500 kbps VBR, keyframe every 2 seconds (48 frames).
- [ ] **4.2.2** Integrate `fast_image_resize` to downscale captured frames from native resolution (e.g., 1080p) to 480p before encoding.
- [ ] **4.2.3** Build the outbound video pipeline on a raw OS thread (normal priority): read frame from video ring → downscale to 480p → VP8 encode → produce encoded chunk.
- [ ] **4.2.4** Implement video packet fragmentation: if an encoded frame (especially keyframes) exceeds the UDP-safe MTU (~1200 bytes), split it into multiple packets using the fragment ID / fragment total fields in the packet header.

### 4.3 Video Network Send/Receive

- [ ] **4.3.1** Send VP8 packets over UDP. Ensure audio packets are always sent before video packets when both are ready simultaneously (audio priority invariant from §8).
- [ ] **4.3.2** On the receive side, implement fragment reassembly: collect all fragments for a frame (matched by timestamp + fragment total), reassemble into a complete encoded frame once all fragments arrive.
- [ ] **4.3.3** Implement selective keyframe NACK: if a keyframe packet (or fragment of one) is detected as lost (gap in sequence numbers for a type=0x02 packet), send a NACK control packet to the sender requesting retransmission. Do not NACK delta frames — just skip them.
- [ ] **4.3.4** Integrate VP8 decoder for incoming streams. Decode reassembled VP8 frames into raw image data for display.

### 4.4 Video Display

- [ ] **4.4.1** Render decoded remote video frames as egui textures. Update at the received framerate.
- [ ] **4.4.2** Implement a 2x2 participant grid layout in egui: local preview in one cell, up to 3 remote feeds in the others. Scale cells to fit the window.
- [ ] **4.4.3** Implement camera on/off toggle: when camera is off, stop sending video packets and show a placeholder (e.g., participant name on solid background) in the local preview cell. Handle receiving side: if no video packets arrive from a peer, show a placeholder for them.

### Milestone 4

> **Two instances connected. You see each other's 480p video feeds in a 2x2 grid AND hear each other's voice. Toggling camera off shows a placeholder. Audio still works perfectly when video is off. Video is noticeably lower priority — if you constrain bandwidth, audio stays clear while video degrades.**

---

## Stage 5: Local Video Recording (Golden Master)

**Goal:** Each participant now records a full-resolution H.264 + AAC MP4 locally, using hardware-accelerated encoding, while simultaneously streaming the 480p live preview. This is the core product deliverable.

### 5.1 Dual Video Ring Buffers

- [ ] **5.1.1** Modify the video capture thread to write into **two** independent SPSC rings: Ring A feeds the VP8 live encoder (already built in Stage 4), Ring B feeds the new H.264 local recorder. Each ring sized for 4–6 frames at full resolution.
- [ ] **5.1.2** Stress-test to confirm Ring B (local recording) never drops frames under normal CPU load. Log ring buffer fill level as a diagnostic metric.

### 5.2 H.264 Hardware Encoder — macOS

- [ ] **5.2.1** Implement FFI bindings to Apple's VideoToolbox framework: `VTCompressionSessionCreate`, `VTCompressionSessionEncodeFrame`, `VTCompressionSessionCompleteFrames`. (Or integrate an existing Rust VideoToolbox wrapper crate if a suitable one exists.)
- [ ] **5.2.2** Configure the VideoToolbox H.264 encoder: High Profile, Level 4.1, 15–20 Mbps VBR, constant frame rate (CFR) at 30 fps, YUV 4:2:0 color space.
- [ ] **5.2.3** Set the GOP (Group of Pictures — the interval between keyframes/I-frames) to 30 frames (= 1-second keyframe interval).
- [ ] **5.2.4** Build the macOS local video encode pipeline on a dedicated raw OS thread: read full-resolution frame from Ring B → submit to VideoToolbox → receive encoded H.264 NALUs (Network Abstraction Layer Units — the chunks of encoded H.264 data) via async callback → pass to MP4 muxer.

### 5.3 H.264 Hardware Encoder — Windows

- [ ] **5.3.1** Implement FFI bindings to Windows Media Foundation's MFT (Media Foundation Transform) H.264 encoder. Alternatively, implement NVENC bindings via NVIDIA's Video Codec SDK if the user has an NVIDIA GPU (with Media Foundation as fallback).
- [ ] **5.3.2** Configure the same H.264 parameters as the macOS path: High Profile, Level 4.1, 15–20 Mbps VBR, CFR 30 fps, 1-second GOP.
- [ ] **5.3.3** Build the Windows local video encode pipeline: same architecture as macOS (Ring B → HW encode → NALUs → MP4 muxer), using the Windows-specific encoder behind `#[cfg(target_os = "windows")]`.

### 5.4 Full fMP4 Muxer (Video + Audio)

- [ ] **5.4.1** Extend the audio-only fMP4 muxer (from Stage 3) to interleave a second track: H.264 video alongside AAC audio. Each fragment contains both audio and video samples for that 1-second window.
- [ ] **5.4.2** Implement CFR (Constant Frame Rate) enforcement: if the camera delivers a frame late or drops one, insert a duplicate of the previous frame to maintain exactly 30 fps in the output. Log when this happens.
- [ ] **5.4.3** Verify fragment flushing: every 1 second, a complete interleaved audio+video fragment is written to disk.
- [ ] **5.4.4** Implement finalization: on clean shutdown, write a standards-compliant `moov` atom so the MP4 is playable everywhere.
- [ ] **5.4.5** Validate the output file in multiple tools: `ffprobe` (check codec, resolution, bitrate, frame count), VLC (playback), Premiere Pro or DaVinci Resolve (import to timeline, verify no frame drops or A/V sync drift).

### 5.5 Recording Integrity

- [ ] **5.5.1** Add ring buffer overflow monitoring: if Ring B (local video) ever reaches >80% capacity, log a critical warning with the fill level. If it overflows, log an error but attempt to recover (do not silently discard frames).
- [ ] **5.5.2** On session end, write `total_frames_captured` and `total_frames_dropped` to `session_metadata.json`. Update the metadata schema to include `video_resolution` reflecting the actual camera resolution used.
- [ ] **5.5.3** Verify crash safety for the full A/V file: kill the process mid-recording, confirm the partial fMP4 is recoverable and playable up to the last flushed fragment.

### Milestone 5

> **Have a two-person call for 60+ seconds, end the call. Open `local_recording.mp4` — it's a 1080p (or camera-native resolution) H.264 video with clear AAC audio. Import it into a video editor (Resolve, Premiere, etc.): the timeline shows correct frame rate, no dropped frames, and A/V stays in sync. The `session_metadata.json` reports `total_frames_dropped: 0`. Meanwhile, the live preview stream was running at 480p VP8 the entire time.**

---

## Stage 6: Full Mesh, Sync & Congestion Control

**Goal:** Scale from 2 participants to the full 3–4 person mesh. Add clock synchronization and sync tone so that multi-cam recordings can be aligned in post. Add congestion detection so the call degrades gracefully instead of breaking.

### 6.1 Multi-Peer Mesh

- [ ] **6.1.1** Extend the host handshake to support multiple guests joining sequentially. On each new guest's `Hello`, the host assigns a new participant ID and sends `PeerJoined` to all existing participants (with the new peer's IP:port).
- [ ] **6.1.2** Implement guest-side peer discovery: on receiving `Welcome` (with existing peer list) or `PeerJoined`, open direct UDP connections to the new peer. Each guest also opens their own UPnP port mapping on join.
- [ ] **6.1.3** Handle mid-session peer departure: when a peer disconnects (BYE or timeout), notify remaining peers, update the UI grid layout, and stop encode/decode for that peer's streams. Do not disrupt the local recording or other peer streams.
- [ ] **6.1.4** Full mesh verification: test with 4 participants. Each sends audio+video to 3 others and receives 3 incoming streams. Verify upload bandwidth stays within the ~1.65 Mbps budget.

### 6.2 Clock Synchronization

- [ ] **6.2.1** Implement NTP-style time sync: after handshake completes, the host and each guest exchange 8 round-trip timestamp packets. Calculate the clock offset between each peer using the median round-trip time (filters out outliers from network jitter).
- [ ] **6.2.2** Store the calculated per-peer clock offset (in milliseconds, relative to the host's clock) in the session state.
- [ ] **6.2.3** Embed session-relative timecodes into the local recording's MP4 metadata track (or as a timecode text track). The timecode starts at 00:00:00:00 when the session begins, based on the host's reference clock.
- [ ] **6.2.4** On session end, generate `sync_timecodes.txt` — a human-readable file listing: session start time (UTC), each participant's name, their clock offset, and the timecode at which recording started.

### 6.3 Audio Sync Tone

- [ ] **6.3.1** Implement sync tone generation: a 1 kHz sine wave, 200 ms duration, at a comfortable volume. Generate it as raw PCM samples at 48 kHz.
- [ ] **6.3.2** After the time sync exchange completes and all peers are connected, the host sends a control packet signaling "play sync tone now." All participants (including host) play the tone through their speakers simultaneously (adjusted for clock offsets).
- [ ] **6.3.3** Ensure the sync tone is captured by each participant's local recording (it enters through the mic and/or is directly mixed into the local recording audio track). This gives editors a visible audio spike to snap-align tracks.

### 6.4 Congestion Detection & Response

- [ ] **6.4.1** Implement per-peer network quality monitoring: calculate rolling RTT (from heartbeat round-trips) and packet loss rate (from sequence number gaps) over a 2-second sliding window.
- [ ] **6.4.2** Implement the degradation ladder (triggered per-peer when loss > threshold): Step 1: reduce outgoing video bitrate to 200 kbps. Step 2: reduce outgoing video framerate to 15 fps. Step 3: reduce outgoing video resolution to 360p. Step 4: stop sending video entirely (audio-only mode).
- [ ] **6.4.3** Implement recovery: when conditions improve (loss drops below threshold for 5+ seconds), step back up the degradation ladder gradually.
- [ ] **6.4.4** Implement auto-resume on transient loss: if a peer's packets stop and then resume within the 5-second heartbeat timeout, seamlessly continue decoding/rendering their streams without re-handshake or user-visible error.

### Milestone 6

> **Four people on a call from different machines. Everyone sees and hears everyone else. Each person's local recording has embedded timecodes relative to the host's clock. After the call, import all four MP4 files into a video editor — the timecodes (and sync tone audio spike) allow frame-accurate alignment across all four tracks. If you artificially throttle one person's network, their outgoing video degrades gracefully while audio remains clear.**

---

## Stage 7: UI Completion & Production Readiness

**Goal:** Replace the developer-grade UI with the final MVP user interface: pre-call setup, in-call controls, post-call summary. Add all remaining lifecycle polish (disk space checks, signal handling, clean UPnP teardown). Produce optimized release binaries for both platforms.

### 7.1 Pre-Call Screen

- [ ] **7.1.1** Implement dropdown selectors for: camera device, microphone device, speaker/headphone device. Changing a selection live-switches the device.
- [ ] **7.1.2** Show a live camera preview (rendering frames from the capture pipeline) on the pre-call screen.
- [ ] **7.1.3** Show a real-time mic input level meter (horizontal bar that reacts to microphone volume).
- [ ] **7.1.4** Implement the headphone prompt: a dismissible modal dialog on first entering the pre-call screen — "HyperZoom requires headphones for best audio quality. Please connect headphones before joining."
- [ ] **7.1.5** Display estimated recording file size based on detected camera resolution (e.g., "~8 GB for 1 hour at 1080p" or "~5 GB for 1 hour at 720p").
- [ ] **7.1.6** Check available disk space on the recording drive. If < 20 GB free, show a warning banner (non-blocking).
- [ ] **7.1.7** Implement the Host button: triggers UPnP port mapping → discovers public IP → displays the public IP:port in a large, copyable text field with a "Copy" button.
- [ ] **7.1.8** Implement the Join button: shows a text input field for the host's IP:port → on submit, initiates the handshake sequence.

### 7.2 In-Call Screen

- [ ] **7.2.1** Finalize the 2x2 video grid: show participant display names overlaid on each cell. Adapt layout for 2 or 3 participants (e.g., 1x2 for 2 people, 2x2 with one empty for 3).
- [ ] **7.2.2** Implement mic mute/unmute toggle button with a clear visual indicator (e.g., red mic icon when muted). When muted, stop sending audio packets but keep local recording running.
- [ ] **7.2.3** Implement camera on/off toggle button. When off, stop sending video packets and show placeholder. Local video recording continues if camera is physically on (just not streamed).
- [ ] **7.2.4** Show an always-visible recording indicator (e.g., red dot + "REC" label) confirming local recording is active.
- [ ] **7.2.5** Implement a toggleable network stats overlay: per-peer RTT (ms), packet loss (%), estimated audio one-way latency (ms), current video bitrate being sent/received.
- [ ] **7.2.6** Implement the End Call button. On click: stop all streams → finalize recording → send BYE → transition to post-call screen.

### 7.3 Post-Call Screen

- [ ] **7.3.1** Display the full file path to the saved `local_recording.mp4`.
- [ ] **7.3.2** Implement "Open Folder" button: opens the session recording directory in Finder (macOS) or Explorer (Windows) using the platform-native shell command.
- [ ] **7.3.3** Display a session summary: total call duration, recording file size, video resolution, total frames captured vs. dropped.

### 7.4 Graceful Lifecycle & Cleanup

- [ ] **7.4.1** Ensure End Call triggers in order: (1) send BYE packets, (2) stop all encode/decode threads, (3) finalize fMP4 (flush remaining fragments + write moov), (4) write session_metadata.json and sync_timecodes.txt, (5) unmap UPnP ports.
- [ ] **7.4.2** Handle OS-level termination signals: register a handler for SIGTERM/SIGINT (macOS/Linux) and `SetConsoleCtrlHandler`/WM_CLOSE (Windows). On signal, execute the same graceful shutdown sequence as End Call.
- [ ] **7.4.3** Add a safety timeout to shutdown: if finalization takes longer than 10 seconds (e.g., HW encoder stalled), force-exit and log a warning. The fMP4 fragments already flushed will still be recoverable.

### 7.5 Cross-Platform Release Build

- [ ] **7.5.1** Verify the macOS build compiles and runs the full end-to-end flow (host, join, call, record, end call, open recording) on both Apple Silicon and Intel Macs.
- [ ] **7.5.2** Verify the Windows build compiles and runs the full end-to-end flow on Windows 10 and/or 11.
- [ ] **7.5.3** Configure release profile in `Cargo.toml`: enable LTO (Link-Time Optimization), set `strip = true`, `opt-level = 3`, `codegen-units = 1`. Build release binaries and verify they are < 30 MB per platform.

### Milestone 7 (Final MVP)

> **The full user story from SPECS.md §11 works end-to-end: 4 friends launch HyperZoom, one hosts, three join by IP:port. They talk for an hour with ultra-low-latency audio and 480p video. They end the call. Each person has a pristine 1080p (or native res) H.264 + AAC MP4 with embedded sync timecodes. They share files, import into DaVinci Resolve, align by timecode/sync tone, and edit a professional podcast. The binary is a single self-contained executable under 30 MB.**

---

## Summary

| Stage | Description | Key Deliverable |
|---|---|---|
| **1** | Project Bootstrap & Audio Pipeline | Mic loopback through egui window |
| **2** | Network Protocol & Live Audio Streaming | Two-person real-time voice call over UDP |
| **3** | Local Audio Recording | AAC + fMP4 local recording alongside live stream |
| **4** | Video Capture & Live Preview Streaming | 480p VP8 video call with 2x2 grid |
| **5** | Local Video Recording (Golden Master) | 1080p H.264 HW-encoded local MP4 |
| **6** | Full Mesh, Sync & Congestion Control | 4-person call with time-aligned recordings |
| **7** | UI Completion & Production Readiness | Polished UI, release binaries, full MVP |
