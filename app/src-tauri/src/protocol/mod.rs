//! protocol/mod.rs — typed frame definitions for the app↔relay protocol.
//!
//! Everything else in the codebase depends only on the types in this file.
//! The upstream Ollalink sound-stream schema is owned by the relay server
//! (server/src/ollalink.js) — the app never talks to Ollalink directly.

use serde::{Deserialize, Serialize};

// ---------- Server → Client events ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerEvent {
    Joined {
        room: String,
        #[serde(rename = "self")] self_participant: Participant,
        participants: Vec<Participant>,
    },
    PeerJoined { peer: Participant },
    PeerLeft { #[serde(rename = "sessionId")] session_id: String },
    /// Header frame for an incoming audio binary. Next WS frame is PCM.
    Audio {
        from: String,
        #[serde(default)]
        lang: String,
        #[serde(default)]
        codec: Option<String>,
        #[serde(rename = "sampleRate", alias = "sample_rate")]
        sample_rate: Option<u32>,
        #[serde(rename = "chunkSeq")]
        chunk_seq: Option<u64>,
        #[serde(default)]
        last: Option<bool>,
        #[serde(rename = "endOfUtterance")]
        end_of_utterance: Option<bool>,
        #[serde(rename = "hasBinary")]
        has_binary: Option<bool>,
        #[serde(rename = "utteranceId")]
        utterance_id: Option<String>,
        #[serde(rename = "audio_b64", alias = "audioB64", default)]
        audio_b64: Option<String>,
    },
    Caption {
        kind: CaptionKind,
        from: String,
        payload: serde_json::Value,
    },
    Error { code: String, message: String },
    Pong { ts: u64 },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub display_name: String,
    pub source_lang: String,
    pub target_lang: String,
    pub joined_at: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionKind {
    Ready,
    CaptionPartial,
    CaptionFinal,
    Translation,
    Error,
    Unknown,
}

// ---------- Joined-waiter registry (for sequenced start_call) ----------

mod joined_registry {
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use tokio::sync::oneshot;

    #[derive(Default)]
    struct RegistryState {
        waiters: HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>,
        completed: HashMap<String, Result<serde_json::Value, String>>,
    }

    static STATE: Lazy<Mutex<RegistryState>> = Lazy::new(|| Mutex::new(RegistryState::default()));

    pub fn register(id: &str, sender: oneshot::Sender<Result<serde_json::Value, String>>) {
        let mut g = STATE.lock();
        if let Some(res) = g.completed.remove(id) {
            let _ = sender.send(res);
        } else {
            g.waiters.insert(id.to_string(), sender);
        }
    }

    pub fn resolve(id: &str, value: serde_json::Value) {
        let mut g = STATE.lock();
        if let Some(tx) = g.waiters.remove(id) {
            let _ = tx.send(Ok(value));
        } else {
            g.completed.insert(id.to_string(), Ok(value));
        }
    }

    pub fn reject(id: &str, err: String) {
        let mut g = STATE.lock();
        if let Some(tx) = g.waiters.remove(id) {
            let _ = tx.send(Err(err));
        } else {
            g.completed.insert(id.to_string(), Err(err));
        }
    }

    /// Cancel a waiter without resolving - used when the socket dies before
    /// the joined event arrives. Prevents timeout-hanging on dead sockets.
    #[allow(dead_code)]
    pub fn cancel(id: &str) {
        let mut g = STATE.lock();
        g.waiters.remove(id);
        g.completed.remove(id);
    }

    /// Cancel ALL waiters (used on socket close). Existing waiters get a
    /// dropped sender, which makes their receivers return Err - exactly the
    /// "early failure" semantics we want for start_call_inner.
    pub fn cancel_all() {
        let mut g = STATE.lock();
        g.waiters.clear();
        g.completed.clear();
    }
}

#[allow(unused_imports)]
pub use joined_registry::{
    register as register_joined_waiter,
    resolve as resolve_joined_waiter,
    reject as reject_joined_waiter,
    cancel as cancel_joined_waiter,
    cancel_all as cancel_all_joined_waiters,
};



#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_joined_registry_normal_order() {
        let id = "test_norm_unique";
        let (tx, rx) = tokio::sync::oneshot::channel();
        register_joined_waiter(id, tx);
        resolve_joined_waiter(id, serde_json::json!({ "ok": true }));
        let res = rx.await.unwrap();
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["ok"], true);
    }

    #[tokio::test]
    async fn test_joined_registry_early_resolve_buffered() {
        let id = "test_early_unique";
        // Server sends joined BEFORE register is called
        resolve_joined_waiter(id, serde_json::json!({ "room": "ROOM1" }));
        let (tx, rx) = tokio::sync::oneshot::channel();
        register_joined_waiter(id, tx);
        let res = rx.await.unwrap();
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["room"], "ROOM1");
    }

    #[tokio::test]
    async fn test_joined_registry_early_reject_buffered() {
        let id = "test_reject_unique";
        reject_joined_waiter(id, "room-full: room is full".into());
        let (tx, rx) = tokio::sync::oneshot::channel();
        register_joined_waiter(id, tx);
        let res = rx.await.unwrap();
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "room-full: room is full");
    }
}

