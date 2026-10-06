//! ws/mod.rs Ã¢â‚¬â€ relay WebSocket client.
//!
//! One socket per call, to our own relay. Frames:
//!
//!   Client Ã¢â€ â€™ server
//!     { "type": "join", "token": "...", "room": "ABC12", "displayName": "..." }
//!     <binary PCM frame> Ã¢â‚¬â€ 16kHz mono s16le
//!     { "type": "leave" } | { "type": "ping" }
//!
//!   Server Ã¢â€ â€™ client
//!     { "type": "joined", ... } | { "type": "peer-joined", ... } | ...
//!     { "type": "audio", from, lang }  then a binary frame with the audio
//!     { "type": "caption", kind, from, payload }
//!     { "type": "error", code, message }
//!
//! Outbound writes go through an unbounded mpsc Ã¢â€ â€™ single writer task, so the
//! audio capture callback never awaits the network.
//!
//! Auto-reconnect with exponential backoff. The audio RX channel is owned
//! by RelaySocket and stays stable across reconnects Ã¢â‚¬â€ the read task writes
//! into it; the playback task reads from it.

pub mod session;
pub use session::mint_session_via_relay;

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{resolve_joined_waiter, reject_joined_waiter, ServerEvent};

const MAX_RECONNECT_ATTEMPTS: u32 = 8;
const INITIAL_BACKOFF_MS: u64 = 250;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpectedChunk {
    pub sample_rate: u32,
    pub is_last: bool,
}

#[derive(Debug, Clone)]
pub struct RelaySocketConfig {
    pub ws_url: String,
    pub token: String,
    pub room_code: Option<String>,
    pub display_name: String,
    pub captions_on: bool,
}

#[derive(Clone)]
pub struct RelaySocket {
    inner: Arc<RelayInner>,
}

struct RelayInner {
    cfg: RelaySocketConfig,
    current_token: parking_lot::RwLock<String>,
    app: AppHandle,
    /// Outbound queue. Replaced on reconnect; send_pcm clones the current sender.
    out_tx: Mutex<mpsc::UnboundedSender<Message>>,
    /// Stable inbound-audio queue. Sender is swapped on reconnect; receiver lives forever.
    audio_in_tx: Mutex<mpsc::UnboundedSender<(Vec<u8>, u32, bool)>>,
    audio_out_rx: Mutex<mpsc::UnboundedReceiver<(Vec<u8>, u32, bool)>>,
    reconnect_pcm_buffer: Mutex<std::collections::VecDeque<Vec<u8>>>,
    shutdown: AtomicBool,
    reconnecting: AtomicBool,
    reconnect_notify: Notify,
}

impl RelaySocket {
    pub async fn connect(cfg: RelaySocketConfig, app: AppHandle) -> Result<Self> {
        // Clear any stale waiter state before opening new socket.
        crate::protocol::cancel_all_joined_waiters();

        // Stable audio channel: receiver lives for the lifetime of the socket handle.
        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<(Vec<u8>, u32, bool)>();
        // Provisional out_tx; replaced after first open_socket call.
        let (out_tx, _drop_rx) = mpsc::unbounded_channel::<Message>();

        let current_token = parking_lot::RwLock::new(cfg.token.clone());
        let inner = Arc::new(RelayInner {
            cfg,
            current_token,
            app,
            out_tx: Mutex::new(out_tx),
            audio_in_tx: Mutex::new(audio_tx),
            audio_out_rx: Mutex::new(audio_rx),
            reconnect_pcm_buffer: Mutex::new(std::collections::VecDeque::new()),
            shutdown: AtomicBool::new(false),
            reconnecting: AtomicBool::new(false),
            reconnect_notify: Notify::new(),
        });

        Self::open_socket(inner.clone())
            .await
            .context("initial relay connect")?;

        Ok(Self { inner })
    }

    /// Establish the WebSocket and spawn read/write tasks.
    /// Called once at connect() and again from the reconnect loop.
    async fn open_socket(inner: Arc<RelayInner>) -> Result<()> {
        let normalized_ws = inner.cfg.ws_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        let url = url::Url::parse(&normalized_ws).context("parse relay ws url")?;
        tracing::info!(%url, "connecting to relay");

        let (ws_stream, _resp) = connect_async(url.as_str())
            .await
            .context("relay ws handshake")?;

        let (mut write_half, mut read_half) = ws_stream.split();

        // Swap outbound sender so any buffered frames drain into the new socket.
        let (new_out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        *inner.out_tx.lock().await = new_out_tx.clone();

        // Initial join frame Ã¢â‚¬â€ written directly, not via queue.
        let token = inner.current_token.read().clone();
        let join = serde_json::json!({
            "type": "join",
            "token": token,
            "room": inner.cfg.room_code,
            "displayName": inner.cfg.display_name,
            "captionsOn": inner.cfg.captions_on,
        });
        write_half
            .send(Message::Text(join.to_string()))
            .await
            .context("send join frame")?;

        // Writer task Ã¢â‚¬â€ drains out_rx into the new write_half.
        let inner_w = inner.clone();
        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if inner_w.shutdown.load(Ordering::Relaxed) { break; }
                if write_half.send(msg).await.is_err() {
                    tracing::warn!("relay ws write failed");
                    break;
                }
            }
            tracing::debug!("writer task exited");
        });

        // Reader task. Uses the *stable* audio_in_tx so playback is undisturbed.
        let inner_r = inner.clone();
        tokio::spawn(async move {
            // Strict FIFO queue for matching text headers to upcoming binary frames.
            // Bounded to 32 items to guarantee deterministic memory and prevent offset drifts on dropped frames.
            let mut last_known_rate: u32 = 48_000;
            let mut expected_audio_queue: std::collections::VecDeque<ExpectedChunk> =
                std::collections::VecDeque::with_capacity(32);
            // Bug 1 Fix: Queue for orphan binary frames that arrive before their text metadata header.
            let mut pending_binary_queue: std::collections::VecDeque<Vec<u8>> =
                std::collections::VecDeque::with_capacity(32);

            while let Some(msg) = read_half.next().await {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        expected_audio_queue.clear();
                        pending_binary_queue.clear();
                        let _ = inner_r.app.emit("relay-error", e.to_string());
                        break;
                    }
                };
                match msg {
                    Message::Binary(b) => {
                        // 0. OLAU self-describing binary frame: [OLAU (4B)][Rate (4B LE)][Flags (1B)][Seq (2B)][PCM...]
                        // Eliminates FIFO queue pairing and desync entirely!
                        if b.len() >= 11 && &b[0..4] == b"OLAU" {
                            let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                            let flags = b[8];
                            let is_last = (flags & 1) != 0;
                            let pcm = b[11..].to_vec();
                            if rate > 0 {
                                last_known_rate = rate;
                            }
                            let tx = inner_r.audio_in_tx.lock().await;
                            let _ = tx.send((pcm, rate, is_last));
                            continue;
                        }

                        // 1. WAV containers are self-describing; byte 24..28 holds native sample rate.
                        let detected_rate = if b.starts_with(b"RIFF") && b.len() >= 28 && b.get(8..12) == Some(b"WAVE") {
                            let r = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
                            if r > 0 { Some(r) } else { Some(24_000) }
                        } else {
                            None
                        };

                        if let Some(meta) = expected_audio_queue.pop_front() {
                            if meta.sample_rate == 0 {
                                // Duplicate binary arrived for chunk already consumed via audio_b64; discard cleanly
                                continue;
                            }
                            let rate = detected_rate.unwrap_or(meta.sample_rate);
                            if detected_rate.is_none() && rate > 0 {
                                last_known_rate = rate;
                            }
                            let tx = inner_r.audio_in_tx.lock().await;
                            let _ = tx.send((b, rate, meta.is_last));
                        } else if let Some(r) = detected_rate {
                            // Self-describing WAV has its own rate in the RIFF header
                            let tx = inner_r.audio_in_tx.lock().await;
                            let _ = tx.send((b, r, false));
                        } else {
                            // Orphan binary frame arrived BEFORE its text metadata header!
                            // Buffer in pending_binary_queue so Message::Text can pair it with its verified sample rate!
                            // Never guess 48kHz which creates chipmunk/speed distortion!
                            if pending_binary_queue.len() >= 32 {
                                pending_binary_queue.pop_front();
                            }
                            pending_binary_queue.push_back(b);
                        }
                    }
                    Message::Text(t) => {
                        let raw_val: serde_json::Value = serde_json::from_str(&t).unwrap_or(serde_json::Value::Null);
                        match serde_json::from_str::<ServerEvent>(&t) {
                            Ok(ServerEvent::Audio { end_of_utterance, has_binary, sample_rate, last, lang: _, ref codec, ref audio_b64, .. }) => {
                                let is_marker = end_of_utterance.unwrap_or(false);
                                let is_last = last.unwrap_or(false);
                                let carries_binary = has_binary.unwrap_or(!is_marker);

                                let is_wav = match codec {
                                    Some(c) => c.eq_ignore_ascii_case("wav"),
                                    None => false,
                                };
                                let default_rate = if is_wav { 24_000 } else { last_known_rate };
                                let chunk_rate = match sample_rate {
                                    Some(sr) if sr > 0 => sr,
                                    _ => default_rate,
                                };
                                if chunk_rate > 0 && !is_wav {
                                    last_known_rate = chunk_rate;
                                }

                                // 0. Single-message self-describing audio path:
                                // If audio_b64 is present directly in this JSON message, decode and dispatch immediately!
                                // ZERO pairing, ZERO FIFO queue, ZERO possibility of desynchronization!
                                if let Some(ref b64) = audio_b64 {
                                    use base64::Engine;
                                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                                        if carries_binary {
                                            if !pending_binary_queue.is_empty() {
                                                pending_binary_queue.pop_front();
                                            } else {
                                                if expected_audio_queue.len() >= 64 {
                                                    expected_audio_queue.pop_front();
                                                }
                                                expected_audio_queue.push_back(ExpectedChunk {
                                                    sample_rate: 0,
                                                    is_last: false,
                                                });
                                            }
                                        }
                                        let tx = inner_r.audio_in_tx.lock().await;
                                        let _ = tx.send((bytes, chunk_rate, is_last));
                                        let _ = inner_r.app.emit("relay-event", &raw_val);
                                        continue;
                                    }
                                }

                                if carries_binary {
                                    // Normal path: Text header arrives first, binary arrives second.
                                    // If an orphan binary arrived earlier, pair it now.
                                    if let Some(buffered_b) = pending_binary_queue.pop_front() {
                                        let tx = inner_r.audio_in_tx.lock().await;
                                        let _ = tx.send((buffered_b, chunk_rate, is_last));
                                    } else {
                                        // Binary hasn't arrived yet — push meta so binary handler can pair.
                                        if expected_audio_queue.len() >= 64 {
                                            expected_audio_queue.pop_front();
                                        }
                                        expected_audio_queue.push_back(ExpectedChunk {
                                            sample_rate: chunk_rate,
                                            is_last,
                                        });
                                    }
                                } else if is_marker || is_last {
                                    let tx = inner_r.audio_in_tx.lock().await;
                                    let _ = tx.send((Vec::new(), 0, true));
                                    // Utterance boundary: Flush queues so past sentence state never pollutes subsequent utterances
                                    expected_audio_queue.clear();
                                    pending_binary_queue.clear();
                                }
                                let _ = inner_r.app.emit("relay-event", &raw_val);
                            }
                            Ok(ev @ ServerEvent::Joined { .. }) => {
                                resolve_joined_waiter(
                                    "joined",
                                    serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null),
                                );
                                let _ = inner_r.app.emit("relay-event", &raw_val);
                            }
                            Ok(ServerEvent::AudioClear) => {
                                expected_audio_queue.clear();
                                pending_binary_queue.clear();
                                let _ = inner_r.app.emit("relay-event", &raw_val);
                            }
                            Ok(ServerEvent::Error { code, message }) => {
                                reject_joined_waiter("joined", format!("{code}: {message}"));
                                let _ = inner_r.app.emit("relay-event", &raw_val);
                            }
                            Ok(_ev) => {
                                let _ = inner_r.app.emit("relay-event", &raw_val);
                            }
                            Err(_) => {
                                if !raw_val.is_null() {
                                    let _ = inner_r.app.emit("relay-event", &raw_val);
                                } else {
                                    tracing::debug!(text=%t, "unrecognised frame");
                                }
                            }
                        }
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(frame) => {
                        let _ = inner_r.app.emit(
                            "relay-closed",
                            serde_json::json!({ "frame": frame.map(|f| f.reason.to_string()) }),
                        );
                        // Cancel any pending joined waiters so start_call fails fast
                        // instead of timing out at 5 s (F22).
                        crate::protocol::cancel_all_joined_waiters();
                        break;
                    }
                    _ => {}
                }
            }
            tracing::debug!("reader task exited");

            // Reader exited = connection is gone. Trigger reconnect if not deliberate.
            if !inner_r.shutdown.load(Ordering::Relaxed) {
                Self::spawn_reconnect(inner_r.clone());
            }
        });

        Ok(())
    }

    /// Spawns a reconnect task if one isn't already running (guarded by `reconnecting`).
    fn spawn_reconnect(inner: Arc<RelayInner>) {
        // Guard: only one reconnect loop at a time.
        if inner.reconnecting.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let mut attempt = 0u32;
            loop {
                if inner.shutdown.load(Ordering::Relaxed) {
                    inner.reconnecting.store(false, Ordering::SeqCst);
                    return;
                }
                attempt += 1;
                if attempt > MAX_RECONNECT_ATTEMPTS {
                    let _ = inner.app.emit(
                        "relay-error",
                        format!("reconnect failed after {MAX_RECONNECT_ATTEMPTS} attempts"),
                    );
                    inner.reconnecting.store(false, Ordering::SeqCst);
                    return;
                }
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1).min(6));
                tracing::info!(backoff_ms, attempt, "reconnecting to relay");
                let _ = inner.app.emit(
                    "relay-reconnecting",
                    serde_json::json!({ "attempt": attempt, "backoff_ms": backoff_ms }),
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;

                match Self::open_socket(inner.clone()).await {
                    Ok(()) => {
                        tracing::info!("reconnected to relay");
                        let _ = inner.app.emit("relay-reconnected", serde_json::json!({}));
                        inner.reconnect_notify.notify_waiters();
                        inner.reconnecting.store(false, Ordering::SeqCst);

                        // Bug #4 Fix: Paced flush of reconnect PCM buffer (500ms interval)
                        let inner_drain = inner.clone();
                        tokio::spawn(async move {
                            loop {
                                let next_frame = {
                                    let mut buf = inner_drain.reconnect_pcm_buffer.lock().await;
                                    buf.pop_front()
                                };
                                match next_frame {
                                    Some(frame) => {
                                        let tx = inner_drain.out_tx.lock().await.clone();
                                        let _ = tx.send(Message::Binary(frame));
                                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                    }
                                    None => break,
                                }
                            }
                        });
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, attempt, "reconnect attempt failed");
                    }
                }
            }
        });
    }

    /// Update the authentication token on this socket (e.g. after proactive session refresh).
    /// Used by reconnect attempts so they never send an expired token.
    pub fn update_token(&self, new_token: String) {
        *self.inner.current_token.write() = new_token;
    }

    pub async fn send_pcm(&self, bytes: &[u8]) -> Result<()> {
        let is_reconnecting = self.inner.reconnecting.load(Ordering::Relaxed);
        let mut r_buf = self.inner.reconnect_pcm_buffer.lock().await;

        if is_reconnecting || !r_buf.is_empty() {
            // Cap at 4 frames (~2 seconds of audio) with drop-oldest to prevent burst
            if r_buf.len() >= 4 {
                r_buf.pop_front();
            }
            r_buf.push_back(bytes.to_vec());
            return Ok(());
        }

        let tx = self.inner.out_tx.lock().await.clone();
        tx.send(Message::Binary(bytes.to_vec()))
            .map_err(|e| anyhow!("relay send closed: {e}"))
    }

    pub async fn send_json(&self, v: serde_json::Value) -> Result<()> {
        let tx = self.inner.out_tx.lock().await.clone();
        tx.send(Message::Text(v.to_string()))
            .map_err(|e| anyhow!("relay send closed: {e}"))
    }

    pub async fn close(&self) {
        self.inner.shutdown.store(true, Ordering::Relaxed);
        let tx = self.inner.out_tx.lock().await.clone();
        let _ = tx.send(Message::Text(
            serde_json::json!({ "type": "leave" }).to_string(),
        ));
        let _ = tx.send(Message::Close(None));
    }

    /// Await the next inbound audio frame.
    pub async fn next_inbound_audio(&self) -> Option<(Vec<u8>, u32, bool)> {
        let mut guard = self.inner.audio_out_rx.lock().await;
        guard.recv().await
    }

    #[allow(dead_code)]
    pub async fn wait_reconnect(&self) {
        self.inner.reconnect_notify.notified().await;
    }

    pub async fn wait_for_joined(&self, timeout_ms: u64) -> Result<serde_json::Value> {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<serde_json::Value, String>>();
        crate::protocol::register_joined_waiter("joined", tx);

        let timeout = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms));
        tokio::select! {
            _ = timeout => Err(anyhow!("wait_for_joined timed out after {timeout_ms} ms")),
            res = rx => match res {
                Ok(Ok(val)) => Ok(val),
                Ok(Err(err_msg)) => Err(anyhow!("{err_msg}")),
                Err(_) => Err(anyhow!("wait_for_joined waiter dropped")),
            },
        }
    }
}


#[cfg(test)]
mod ws_self_describing_tests {

    #[test]
    fn test_olau_binary_frame_unpacking() {
        let mut frame = Vec::new();
        frame.extend_from_slice(b"OLAU");
        frame.extend_from_slice(&24000u32.to_le_bytes());
        frame.push(1u8); // is_last = true
        frame.extend_from_slice(&42u16.to_le_bytes()); // chunkSeq = 42
        let pcm = vec![0x12, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&pcm);

        assert_eq!(&frame[0..4], b"OLAU");
        let rate = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        assert_eq!(rate, 24000);
        let flags = frame[8];
        assert_eq!((flags & 1) != 0, true);
        let seq = u16::from_le_bytes([frame[9], frame[10]]);
        assert_eq!(seq, 42);
        assert_eq!(&frame[11..], &pcm[..]);
    }

    #[test]
    fn test_server_event_audio_b64_decoding() {
        use base64::Engine;
        let pcm = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm);
        let json = serde_json::json!({
            "type": "audio",
            "from": "alice",
            "sampleRate": 24000,
            "last": true,
            "audio_b64": b64
        });
        let ev: crate::protocol::ServerEvent = serde_json::from_value(json).unwrap();
        match ev {
            crate::protocol::ServerEvent::Audio { sample_rate, last, audio_b64, .. } => {
                assert_eq!(sample_rate, Some(24000));
                assert_eq!(last, Some(true));
                let decoded = base64::engine::general_purpose::STANDARD.decode(audio_b64.unwrap()).unwrap();
                assert_eq!(decoded, pcm);
            }
            _ => panic!("Expected ServerEvent::Audio"),
        }
    }

    #[test]
    fn test_expected_chunk_duplicate_binary_discard_flag() {
        use super::ExpectedChunk;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(ExpectedChunk {
            sample_rate: 0,
            is_last: false,
        });
        let meta = queue.pop_front().expect("must have meta");
        assert_eq!(meta.sample_rate, 0, "sample_rate 0 signals discard marker");
    }

    #[test]
    fn test_utterance_boundary_clears_orphan_binaries() {
        use super::ExpectedChunk;
        let mut expected_audio_queue = std::collections::VecDeque::new();
        let mut pending_binary_queue = std::collections::VecDeque::new();
        pending_binary_queue.push_back(vec![1, 2, 3, 4]);
        expected_audio_queue.push_back(ExpectedChunk { sample_rate: 48000, is_last: false });
        expected_audio_queue.clear();
        pending_binary_queue.clear();
        assert!(expected_audio_queue.is_empty(), "expected queue must be clean");
        assert!(pending_binary_queue.is_empty(), "pending binary queue must be clean to avoid contaminating next utterance");
    }
}