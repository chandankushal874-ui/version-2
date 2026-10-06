# Checkpoint: 2026-10-07 (Most Stable Release - Full Utterance & Persona Synchronization)

## Overview
This checkpoint marks the benchmark release of **Ollalink Translate**, resolving four high-impact audio scheduling and translation routing bottlenecks.
Extensively tested and confirmed in bidirectional calling across the **Windows Native App (.exe)** and **Mobile Browser Web Client (LAN: http://10.31.168.202:1420)**.

## Core Issues Resolved
1. **Sentence Ending Cut-off Eliminated**:
   - Expanded Web Audio scheduling lookahead from `1.50s` to `8.0s` in `app/ui/src/bridge.ts` across WAV and PCM playback routines.
   - Long, compound utterances no longer experience backward time clamping or sentence truncation.

2. **Multilingual Hearing Lane Fallback Eradicated**:
   - Removed rogue secondary Google Translate and `/v1/tts/speak` injections in both `server/src/server.js` and `app/ui/src/main.ts`.
   - International target lanes (German, Chinese, Spanish, French, Russian, Portuguese, Arabic) now route cleanly through Ollalink's real-time 48kHz neural Sound-Stream without defaulting or reverting to Hindi.

3. **Audio Collision & Choppiness Fixed**:
   - Eliminated dual-stream collisions where rogue TTS audio was injected over the native 48kHz sound-stream. Playback is smooth, pristine, and continuous.

4. **Dynamic Voice Persona Switching**:
   - Validated and enabled immediate upstream reconfiguration on voice persona changes in `server/src/server.js`.
   - All 5 neural voice personas (`nh-m01`, `nh-f01`, `dhvaani-ramesh`, `dhvaani-male`, `dhvaani-female`) and delivery tones switch dynamically mid-call.

## Verification
- **Server Test Suite**: 141 / 141 passed (`npm test`)
- **Rust Backend Core**: 23 / 23 passed (`cargo test`)
- **Frontend App Build**: 0 errors (`tsc && vite build`)
- **Cross-Platform Parity**: Verified between Desktop Exe and Mobile Web Client over LAN.
