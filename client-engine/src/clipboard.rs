// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Clipboard sync, the client's side, platform-independent: the
//! platform gives a [`ClipboardBackend`] (its own clipboard), this does the
//! protocol, the same one remoted speaks. Text and PNG, both ways.
//!
//! - **Local → host:** `poll` (every ~100 ms) notices a new local clipboard
//!   through the backend's change counter, reads it and sends a
//!   `ClipboardOffer` with each item inline when it fits in one chunk. A
//!   `ClipboardRequest` for the current serial is answered with chunks.
//! - **Host → local:** an offer's inline items are taken as they are, the rest
//!   is requested; when every item is complete, the backend writes the lot. That
//!   write's own change number is remembered, so it is not sent back.

use crate::msg::{self, ClipFormat, ClipIncoming, ClipItem, ClipboardData, ClipboardOffer, Msg};
use std::sync::{Arc, Mutex};

/// The platform's clipboard, restricted to what Remote syncs.
pub trait ClipboardBackend: Send + Sync {
    /// A number that changes whenever the clipboard's contents change.
    fn sequence(&self) -> u32;
    /// What is on it now: text (UTF-8) and PNG, when present.
    fn read(&self) -> Vec<(ClipFormat, Vec<u8>)>;
    /// What `read` returns is sensitive (a password manager's copy): the host
    /// keeps it out of its clipboard history.
    fn concealed(&self) -> bool {
        false
    }
    /// Replaces it (`concealed`: don't keep it in a clipboard history). Returns
    /// the sequence number after the write.
    fn write(&self, items: Vec<(ClipFormat, Vec<u8>)>, concealed: bool) -> u32;
}

type Incoming = Option<ClipIncoming>;

struct State {
    /// Our last offer: serial and its items.
    out: Option<(u32, Vec<(ClipFormat, Vec<u8>)>)>,
    next_serial: u32,
    /// The host's offer being assembled.
    inc: Incoming,
    /// The sequence number after our own last write (never offered back).
    own_seq: Option<u32>,
    seen_seq: Option<u32>,
}

pub struct ClipSync {
    backend: Arc<dyn ClipboardBackend>,
    send: Box<dyn Fn(Msg) + Send + Sync>,
    st: Mutex<State>,
}

impl ClipSync {
    pub fn new(backend: Arc<dyn ClipboardBackend>, send: Box<dyn Fn(Msg) + Send + Sync>) -> ClipSync {
        ClipSync { backend, send, st: Mutex::new(State { out: None, next_serial: 1, inc: None, own_seq: None, seen_seq: None }) }
    }

    /// Looks for a local change; offers it to the host. Call about every 100 ms
    /// while connected.
    pub fn poll(&self) {
        let seq = self.backend.sequence();
        let mut st = self.st.lock().unwrap();
        let first = st.seen_seq.is_none();
        if st.seen_seq == Some(seq) {
            return;
        }
        st.seen_seq = Some(seq);
        // What was on the clipboard when we started isn't "new": leave it.
        if first || st.own_seq == Some(seq) {
            return;
        }
        drop(st);
        let items = self.backend.read();
        if items.is_empty() {
            return;
        }
        self.offer_as(items, self.backend.concealed());
    }

    /// Offers `items` to the host as the new clipboard (also the script's
    /// "clipboard set" after it writes locally).
    pub fn offer(&self, items: Vec<(ClipFormat, Vec<u8>)>) {
        self.offer_as(items, false);
    }

    fn offer_as(&self, items: Vec<(ClipFormat, Vec<u8>)>, concealed: bool) {
        let mut st = self.st.lock().unwrap();
        let serial = st.next_serial;
        st.next_serial += 1;
        let infos = items
            .iter()
            .map(|(f, d)| ClipItem {
                format: f.clone(),
                size: d.len() as u64,
                inline: (d.len() <= msg::CLIPBOARD_CHUNK).then(|| d.clone()),
            })
            .collect();
        st.out = Some((serial, items));
        drop(st);
        (self.send)(Msg::ClipboardOffer(ClipboardOffer { serial, items: infos, concealed }));
    }

    /// What the local clipboard holds now (the script's `clipboard-show`).
    pub fn peek(&self) -> Vec<(ClipFormat, Vec<u8>)> {
        self.backend.read()
    }

    /// Writes `items` to the local clipboard without offering them back.
    pub fn set_local(&self, items: Vec<(ClipFormat, Vec<u8>)>, concealed: bool) {
        let seq = self.backend.write(items, concealed);
        let mut st = self.st.lock().unwrap();
        st.own_seq = Some(seq);
        st.seen_seq = Some(seq);
    }

    /// A clipboard message from the host.
    pub fn on_msg(&self, m: &Msg) {
        match m {
            Msg::ClipboardOffer(o) => {
                let Some((inc, asks)) = ClipIncoming::from_offer(o, |_| true) else {
                    self.st.lock().unwrap().inc = None;
                    return;
                };
                for format in asks {
                    (self.send)(Msg::ClipboardRequest { serial: o.serial, format });
                }
                self.st.lock().unwrap().inc = Some(inc);
                self.finish();
            }
            Msg::ClipboardRequest { serial, format } => {
                let st = self.st.lock().unwrap();
                let Some((s, items)) = &st.out else { return };
                if s != serial {
                    return;
                }
                if let Some((_, data)) = items.iter().find(|(f, _)| f == format) {
                    let chunks = ClipboardData::chunks(*serial, format, data);
                    drop(st);
                    for c in chunks {
                        (self.send)(c);
                    }
                }
            }
            Msg::ClipboardData(d) => {
                if let Some(inc) = self.st.lock().unwrap().inc.as_mut() {
                    inc.add(d);
                }
                self.finish();
            }
            _ => {}
        }
    }

    /// Every item complete: it becomes the local clipboard.
    fn finish(&self) {
        let mut st = self.st.lock().unwrap();
        if !st.inc.as_ref().is_some_and(ClipIncoming::is_complete) {
            return;
        }
        let inc = st.inc.take().unwrap();
        drop(st);
        let (serial, concealed, items) = (inc.serial, inc.concealed, inc.into_items());
        eprintln!("remote-client: clipboard from the host (offer {serial}): {} item(s), {} bytes", items.len(), items.iter().map(|i| i.1.len()).sum::<usize>());
        self.set_local(items, concealed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        seq: Mutex<u32>,
        items: Mutex<Vec<(ClipFormat, Vec<u8>)>>,
        concealed: Mutex<bool>,
    }
    impl ClipboardBackend for Fake {
        fn sequence(&self) -> u32 {
            *self.seq.lock().unwrap()
        }
        fn read(&self) -> Vec<(ClipFormat, Vec<u8>)> {
            self.items.lock().unwrap().clone()
        }
        fn concealed(&self) -> bool {
            *self.concealed.lock().unwrap()
        }
        fn write(&self, items: Vec<(ClipFormat, Vec<u8>)>, _: bool) -> u32 {
            *self.items.lock().unwrap() = items;
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        }
    }

    fn rig() -> (Arc<Fake>, ClipSync, Arc<Mutex<Vec<Msg>>>) {
        let fake = Arc::new(Fake::default());
        let sent = Arc::new(Mutex::new(vec![]));
        let s2 = sent.clone();
        (fake.clone(), ClipSync::new(fake, Box::new(move |m| s2.lock().unwrap().push(m))), sent)
    }

    #[test]
    fn a_local_change_is_offered_and_its_big_item_served_in_chunks() {
        let (fake, sync, sent) = rig();
        sync.poll(); // the clipboard at start is left alone
        let big = vec![7u8; msg::CLIPBOARD_CHUNK + 5];
        *fake.items.lock().unwrap() = vec![(ClipFormat::Text, b"hi".to_vec()), (ClipFormat::Png, big.clone())];
        *fake.seq.lock().unwrap() += 1;
        sync.poll();
        let Msg::ClipboardOffer(o) = sent.lock().unwrap().remove(0) else { panic!("no offer") };
        assert_eq!(o.items[0].inline.as_deref(), Some(&b"hi"[..]));
        assert!(o.items[1].inline.is_none() && o.items[1].size == big.len() as u64);
        sync.on_msg(&Msg::ClipboardRequest { serial: o.serial, format: ClipFormat::Png });
        assert_eq!(sent.lock().unwrap().len(), 2);
        sync.poll(); // unchanged: nothing more
        assert_eq!(sent.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_password_managers_copy_is_offered_concealed() {
        let (fake, sync, sent) = rig();
        sync.poll();
        *fake.items.lock().unwrap() = vec![(ClipFormat::Text, b"hunter2".to_vec())];
        *fake.concealed.lock().unwrap() = true;
        *fake.seq.lock().unwrap() += 1;
        sync.poll();
        let Msg::ClipboardOffer(o) = sent.lock().unwrap().remove(0) else { panic!("no offer") };
        assert!(o.concealed);
    }

    #[test]
    fn a_host_offer_is_requested_assembled_written_and_not_sent_back() {
        let (fake, sync, sent) = rig();
        sync.poll();
        let data = vec![9u8; 10];
        sync.on_msg(&Msg::ClipboardOffer(ClipboardOffer {
            serial: 4,
            items: vec![
                ClipItem { format: ClipFormat::Text, size: 2, inline: Some(b"ok".to_vec()) },
                ClipItem { format: ClipFormat::Png, size: 10, inline: None },
            ],
            concealed: false,
        }));
        assert!(matches!(sent.lock().unwrap()[0], Msg::ClipboardRequest { serial: 4, format: ClipFormat::Png }));
        assert!(fake.items.lock().unwrap().is_empty());
        for m in ClipboardData::chunks(4, &ClipFormat::Png, &data) {
            sync.on_msg(&m);
        }
        assert_eq!(fake.items.lock().unwrap().len(), 2);
        sync.poll();
        assert_eq!(sent.lock().unwrap().len(), 1, "our own write was offered back");
    }
}
