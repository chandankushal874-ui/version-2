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
            #[derive(Debug, Clone, Copy)]
            struct ExpectedChunk {
                sample_rate: u32,
                is_last: bool,
            }
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
                        // 1. WAV containers are self-describing; byte 24..28 holds native sample rate.
                        let detected_rate = if b.starts_with(b"RIFF") && b.len() >= 28 && b.get(8..12) == Some(b"WAVE") {
                            let r = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
                            if r > 0 { Some(r) } else { Some(24_000) }
                        } else {
                            None
                        };

                        if let Some(meta) = expected_audio_queue.pop_front() {
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
                            Ok(ServerEvent::Audio { end_of_utterance, has_binary, sample_rate, last, ref lang, ref codec, .. }) => {
                                let is_marker = end_of_utterance.unwrap_or(false);
                                let is_last = last.unwrap_or(false);
                                let carries_binary = has_binary.unwrap_or(!is_marker);

                                let is_wav = match codec {
                                    Some(c) => c.eq_ignore_ascii_case("wav"),
                                    None => false,
                                };
                                let is_multilingual = {
                                    let l = lang.to_lowercase();
                                    ["es", "fr", "zh", "de", "ar", "pt", "ru", "kn"].iter().any(|&prefix| l.starts_with(prefix))
                                };
                                let default_rate = if is_wav || is_multilingual { 24_000 } else { last_known_rate };
                                let chunk_rate = match sample_rate {
                                    Some(sr) if sr > 0 => sr,
                                    _ => default_rate,
                                };
                                if chunk_rate > 0 && !is_wav {
                                    last_known_rate = chunk_rate;
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
                                    // B-04 Fix: Only clear meta queue. Don't clear pending_binary_queue —
                                    // it may contain the final audio chunk that hasn't been paired yet.
                                    expected_audio_queue.clear();
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

