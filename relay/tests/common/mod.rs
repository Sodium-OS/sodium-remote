// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! A minimal rendezvous client for the tests and the live probe (examples/probe.rs).
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use remote_rendezvous::proto::{Role, auth_message, b64, paired_list_message, unb64};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

pub fn new_key() -> SigningKey {
    SigningKey::generate(&mut rand::rngs::OsRng)
}

pub fn id_of(key: &SigningKey) -> String {
    b64(key.verifying_key().as_bytes())
}

/// The request a host sends to upload its paired-client list (for the tests' server, `test.local`).
pub fn signed_list(key: &SigningKey, seq: u64, clients: &[&str]) -> Value {
    signed_list_for("test.local", key, seq, clients)
}

/// The same, signed for the server named `server`.
pub fn signed_list_for(server: &str, key: &SigningKey, seq: u64, clients: &[&str]) -> Value {
    let payload = json!({ "v": 2, "host": id_of(key), "server": server, "seq": seq, "clients": clients }).to_string();
    let sig = key.sign(&paired_list_message(payload.as_bytes()));
    json!({ "t": "paired_list", "list": b64(payload.as_bytes()), "sig": b64(&sig.to_bytes()) })
}

pub struct Dev<S> {
    pub key: SigningKey,
    pub id: String,
    ws: WebSocketStream<S>,
    next_id: u64,
    events: VecDeque<Value>,
    /// The server's challenge, as it came.
    pub challenge: Value,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Dev<S> {
    /// Answers the challenge with `signer`'s signature for `key`'s ID (they differ only
    /// when a test wants a bad signature). Returns the device and the server's reply.
    pub async fn auth_with(ws: WebSocketStream<S>, key: SigningKey, signer: &SigningKey, role: Role)
        -> (Self, Value) {
        Self::auth_full(ws, key, signer, role, None).await
    }

    /// Answers the challenge, with an enrolment token when given.
    pub async fn auth_token(ws: WebSocketStream<S>, key: SigningKey, role: Role, token: Option<&str>) -> (Self, Value) {
        let signer = key.clone();
        Self::auth_full(ws, key, &signer, role, token).await
    }

    async fn auth_full(ws: WebSocketStream<S>, key: SigningKey, signer: &SigningKey, role: Role, token: Option<&str>)
        -> (Self, Value) {
        let id = id_of(&key);
        let mut dev = Dev { key, id, ws, next_id: 1, events: VecDeque::new(), challenge: Value::Null };
        let ch = dev.recv().await;
        assert_eq!(ch["t"], "challenge", "{ch}");
        dev.challenge = ch.clone();
        let nonce = unb64(ch["nonce"].as_str().unwrap()).unwrap();
        let server = ch["server"].as_str().unwrap();
        let sig = signer.sign(&auth_message(server, role, &nonce));
        let mut auth = json!({ "t": "auth", "device": dev.id, "role": role, "sig": b64(&sig.to_bytes()) });
        if let Some(t) = token {
            auth["token"] = t.into();
        }
        dev.send(&auth).await;
        let reply = dev.recv().await;
        (dev, reply)
    }

    pub async fn auth(ws: WebSocketStream<S>, key: SigningKey, role: Role) -> (Self, Value) {
        let signer = key.clone();
        Self::auth_with(ws, key, &signer, role).await
    }

    async fn send(&mut self, v: &Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.expect("send");
    }

    /// The next JSON frame from the server; Value::Null once the connection is closed.
    async fn recv(&mut self) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await
                .expect("timed out waiting for the server");
            match msg {
                Some(Ok(Message::Text(t))) => return serde_json::from_str(t.as_str()).expect("json"),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Value::Null,
                Some(Ok(_)) => continue,
            }
        }
    }

    /// Sends a request and returns its reply (`result` or `error`), keeping any
    /// events that arrive meanwhile for `event`.
    pub async fn request(&mut self, mut req: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = id.into();
        self.send(&req).await;
        loop {
            let v = self.recv().await;
            if v.is_null() {
                return v;
            }
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                return v;
            }
            self.events.push_back(v);
        }
    }

    /// The next event of kind `t`, skipping (and dropping) others.
    pub async fn event(&mut self, t: &str) -> Value {
        if let Some(i) = self.events.iter().position(|e| e["t"] == t) {
            return self.events.remove(i).unwrap();
        }
        loop {
            let v = self.recv().await;
            if v.is_null() || v["t"] == t {
                return v;
            }
        }
    }
}

pub fn temp_dir(tag: &str) -> PathBuf {
    let mut b = [0u8; 6];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut b);
    std::env::temp_dir().join(format!("remote-rendezvous-{tag}-{}", b64(&b)))
}
