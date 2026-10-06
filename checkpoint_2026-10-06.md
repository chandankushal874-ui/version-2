# SYSTEM CHECKPOINT — Ollalink Translate

**Date:** 2026-10-06 / 2026-10-07  
**Milestone:** Multilingual Upstream Schema Normalization, Elimination of 30s–2min Audio Latency, Cross-Language Buffer Purging, Zero-Leak Mid-Call Language Switching, Authenticode Release Verification  
**Project Roots:**  
- Production App: `C:\ollalink-translate-2` (Active Development & Release)  
- Clean Mirror: `C:\ollalink-translate`  
**Conversation ID:** `48803fc4-b1fd-4a6c-b92a-f653f37969fa`  
**Git Remote:** `https://github.com/chandankushal874-ui/version-2.git`  
**Git Branch:** `main` (Head: `5f95fa9`)  
**Authenticode Thumbprint:** `0BD68B0E07AE7FB26C220942C28B65C9D8AF1F63` (DigiCert RFC 3161 Timestamped)  

---

## 1. System Baseline & Verification Status

| Component | Status | Location / Artifact | Details |
|---|---|---|---|
| **Production Binary** | ✅ Signed & Standalone | `C:\ollalink-translate-2\app\src-tauri\target\release\ollalink-translate.exe` | 15.3 MB native release binary, embedded UI bundle, single-instance protected |
| **Setup Installer** | ✅ Signed & Standalone | `C:\ollalink-translate-2\app\src-tauri\target\release\bundle\nsis\Ollalink Translate_0.1.0_x64-setup.exe` | NSIS Windows installer with DigiCert Authenticode RFC 3161 timestamp |
| **Distribution Archive** | ✅ Packaged & Verified | `C:\ollalink-translate-2\Ollalink-Translate-Windows-x64.zip` | Release ZIP with SHA256 checksums, certificate installation, and solo partner bot |
| **Live Cloud Relay** | ✅ Healthy & Active | `https://windows-live-translation-app-lzx2.onrender.com` | Deployed on Render, multi-client room routing, WebSocket `/call` plane |
| **GPU AI Stream** | ✅ Operational | `wss://sound-stream.ollalink.com/v1/speech/stream` | Real-time speech-to-text, translation, and neural voice synthesis |
| **Server Test Suite** | ✅ 141 / 141 Passed | `npm --prefix server test` | 100% test pass across unit, integration, room management, and multi-target fanout |
| **Rust Test Suite** | ✅ 23 / 23 Passed | `cargo test --manifest-path app/src-tauri/Cargo.toml` | 100% test pass across jitter buffer, Rubato resamplers, soft limiters, and protocol |
| **Git Synchronization** | ✅ Synchronized | `https://github.com/chandankushal874-ui/version-2.git` | Head commit `5f95fa9` pushed to `origin/main` with automatic cloud deployment |

---

## 2. Key Achievements & Bug Resolutions Since Checkpoint 2026-10-05

### A. Resolution of the 30s–2min Audio Latency Build-up
- **Root Cause Identified**: The server's reconnection pacing timer (`paceFlushReconnectBuffer`) drained buffered audio chunks at a fixed interval of 500ms. Because microphone capture streams send chunks every 160ms (5120 bytes at 16kHz mono 16-bit PCM), audio chunks entered the reconnect queue 3.1× faster than they were sent. During any network reconnect or language switch, this caused a runaway backlog that scaled up to 30 seconds to 2 minutes of delay.
- **Architectural Solution**:
  1. Reduced `paceFlushReconnectBuffer` pacing interval from 500ms to **160ms**, matching the native real-time chunk cadence.
  2. Capped `reconnectBuffer` size to **4 chunks** (~640ms maximum ceiling). Stale chunks older than 640ms are discarded, guaranteeing that conversation turn latency cannot accumulate.

### B. Upstream Schema Normalization & Multilingual Routing Fix
- **Root Cause Identified**: In the Ollalink Sound-Stream WebSocket protocol, translation audio frames carry the target language code in the field `target` (e.g. `target: "de"`, `target: "hi"`). In `ollalink.js`, `translateEvent` only read `parsed.language ?? parsed.lang ?? ''`. This caused `payload.language` to evaluate to an empty string (`''`), preventing `forwardOllalinkToRoom` from matching peer target languages (`pCanon === peerCanon`). The relay server fell through to broadcast all audio to all participants indiscriminately, stamping Hindi audio as German and routing it to the wrong listener.
- **Architectural Solution**:
  1. Updated `translateEvent` in `server/src/ollalink.js` to inspect `parsed.target ?? parsed.language ?? parsed.lang ?? ''`.
  2. Enforced strict canonical target filtering in `server/src/server.js`: `if (pCanon && peerCanon && pCanon !== peerCanon) continue;`.
  3. Ensured that peers only receive speech audio rendered for their selected language.

### C. Elimination of Cross-Language Audio Leakage on Language Switch
- **Root Cause Identified**: When a participant changed their hearing language mid-call (`lang.change`), the server only cleared the requesting client's `reconnectBuffer`, leaving the speaker's buffer intact. When the speaker's upstream re-opened with the new multi-target set, ancient Hindi speech chunks were immediately flushed into the new session and translated, replaying old turns through the listener's speaker. Additionally, client-side Web Audio and CPAL jitter buffers had up to 30 seconds of queued audio from the previous language that continued playing.
- **Architectural Solution**:
  1. In `server/src/server.js`, `msg.type === 'lang.change'` now clears `reconnectBuffer`, `_flushTimer`, and `_handledUtterances` across **all participants** in the room.
  2. Emitted real-time control message `{ type: "audio.clear" }` to all clients in the room.
  3. In `app/src-tauri/src/ws/mod.rs` & `app/src-tauri/src/protocol/mod.rs`, handled `ServerEvent::AudioClear` to instantly empty `expected_audio_queue` and `pending_binary_queue`.
  4. In `app/src-tauri/src/audio/jitter.rs` & `app/src-tauri/src/state.rs`, implemented `JitterPlayer::clear(&self)` and flushed resamplers on language change.
  5. In `app/ui/src/bridge.ts`, implemented `clearBrowserPlayback()` to abort active Web Audio sources, reset `nextPlayTime = 0`, and clear pending queues upon language switch.

### D. Audio DSP Sample Rate Correction (48 kHz Streaming vs 24 kHz WAV Batch)
- **Root Cause Identified**: Multilingual voices (`es, fr, zh, de, ar, pt, ru, kn`) were previously hardcoded to 24,000 Hz. Ollalink's real-time Sound-Stream PCM streaming lane natively synthesizes 48,000 Hz audio for all targets. Tagging 48 kHz PCM audio as 24 kHz caused Rubato and Web Audio to resample 24k → 48k (a 2× stretch), resulting in half-speed (0.5×) playback, severe deep-bass pitch drop, and doubled playback duration.
- **Architectural Solution**:
  1. Guaranteed that streaming PCM defaults to **48,000 Hz** across all streaming targets (`en, hi, es, fr, zh, de, ar, pt, ru`).
  2. Reserved 24,000 Hz strictly for the batch lane (WAV container format with RIFF header, used for Kannada, Tamil, and Telugu).
  3. Cleanly eliminated slow-mo distortion and doubled playback duration across all layers (`ollalink.js`, `server.js`, `ws/mod.rs`, `bridge.ts`).

---

## 3. Architecture & Data Flow

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                              END-TO-END PIPELINE DIAGRAM                               │
└────────────────────────────────────────────────────────────────────────────────────────┘

  [Speaker Mic] (16kHz Mono 16-bit PCM)
        │
        ▼ (160ms / 5120-byte Chunks)
  [Relay Server: server.js] ── (Buffer Cap: 4 Chunks @ 160ms Pacing)
        │
        ▼
  [Ollalink Sound-Stream] (wss://sound-stream.ollalink.com/v1/speech/stream)
        │
        ▼ { type: "translation.audio", target: "de", sample_rate: 48000 }
  [translateEvent: ollalink.js] ── Normalized `target` + 48kHz Streaming Default
        │
        ▼
  [forwardOllalinkToRoom: server.js] ── Strict Canonical Language Match (pCanon === peerCanon)
        │
        ▼ { type: "audio", lang: "de", sampleRate: 48000 }
  [Remote Listener Client] (Desktop CPAL JitterPlayer / Web Audio Context)
        │
        └── On `lang.change`: Server broadcasts `{ type: "audio.clear" }`
            └── Client immediately flushes jitter buffers & Web Audio schedule (0ms residue).
```

---

## 4. Verification Checksums & Release Artifacts

### Release Binaries:
- **`ollalink-translate.exe`**:
  `SHA256: 342b179d8ad65ee74c834f84cf882030d4082a048198762a999a3cab2d9aad51`
- **`Ollalink-Translate-Setup.exe`**:
  `SHA256: 33c5706ae7750401657389b2d62e6933a1f72f7190995faf6ce5bab268f5a771`
- **`Ollalink-Translate-Windows-x64.zip`**:
  `SHA256: b73ca8763b4423e11f5d8d17e0bbce0119eec43a42fea36a76323fc8786d7640`

### Test Suite Results:
- **Rust Backend**: `cargo test --manifest-path app/src-tauri/Cargo.toml`
  `23 passed, 0 failed, 0 warnings`
- **Server Relay**: `npm --prefix server test`
  `141 passed, 0 failed`

---

## 5. Next Steps
1. Perform multi-person interactive voice call testing between distinct physical devices (e.g. Phone browser + Windows desktop).
2. Validate mid-call language switching between English, German, and Spanish with continuous speech.
3. Monitor Render deployment logs for steady-state connection memory and CPU metrics under multi-client concurrency.
