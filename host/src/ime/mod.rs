// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The IME bridge: the Mac's input method composes, the fcitx5 add-on
//! `remote/fcitx5-remote` shows it in the focused app.
//!
//! - On connect remoted calls `Activate` (every field switches to the add-on's
//!   `remote` engine); on disconnect, and while a lost session is held, `Deactivate`
//!   (every field gets its own engine back, so typing at the desk works).
//! - The client's marked text goes to `SetPreedit`, its insertText to `Commit`.
//! - Back to the client: the add-on's `CaretRect`, `FocusChanged(focused, password)`
//!   and `Reset`, and the plugin's CARET (capture-ipc 137), which is the only caret
//!   source for text-input-v3 apps. Its purpose 8 (password) and 9 (PIN) mean
//!   secure input on the client.
//! - If fcitx5 restarts mid-session, the add-on comes back inactive; remoted
//!   activates it again when its bus name reappears.
//! - Without the add-on (not installed yet), all of this is skipped quietly and
//!   committed text falls back to typed keys on the client's side.

use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.sodiumos.Remote.Ime",
    default_service = "org.sodiumos.Remote.Ime",
    default_path = "/org/sodiumos/Remote/Ime"
)]
pub trait Ime {
    fn activate(&self) -> zbus::Result<()>;
    fn deactivate(&self) -> zbus::Result<()>;
    fn set_preedit(&self, text: &str, cursor: i32) -> zbus::Result<bool>;
    fn commit(&self, text: &str) -> zbus::Result<bool>;
    #[zbus(property)]
    fn active(&self) -> zbus::Result<bool>;
    #[zbus(signal)]
    fn caret_rect(&self, x: i32, y: i32, w: i32, h: i32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn focus_changed(&self, focused: bool, password: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn reset(&self) -> zbus::Result<()>;
}

/// What the client hears about text input on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    /// The caret in REMOTE-1's pixels; the client puts its candidate window there.
    Caret { x: i32, y: i32, w: i32, h: i32 },
    /// A text field gained or lost focus; `secure` = a password or PIN field.
    Focus { focused: bool, secure: bool },
    /// The app ended the composition: the client discards its marked text.
    Reset,
}

/// The plugin's CARET (capture-ipc 137, the media module's `MediaEvent::Caret`):
/// the text-input-v3 caret in REMOTE-1's pixels and the field's content purpose.
pub fn from_plugin_caret(visible: bool, purpose: u32, x: i32, y: i32, w: i32, h: i32) -> Vec<ImeEvent> {
    let mut out = vec![ImeEvent::Focus { focused: visible || purpose != 0, secure: purpose == 8 || purpose == 9 }];
    if visible {
        out.push(ImeEvent::Caret { x, y, w, h });
    }
    out
}

pub struct ImeBridge {
    proxy: ImeProxy<'static>,
    session: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ImeBridge {
    /// Listens to the add-on's signals for the life of the daemon.
    pub async fn new(conn: &Connection, tx: mpsc::UnboundedSender<ImeEvent>) -> Result<Self> {
        let proxy = ImeProxy::new(conn).await?;
        let session = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p, t) = (proxy.clone(), tx.clone());
        tokio::spawn(async move {
            let Ok(mut s) = p.receive_caret_rect().await else { return };
            while let Some(sig) = s.next().await {
                if let Ok(a) = sig.args() {
                    let _ = t.send(ImeEvent::Caret { x: a.x, y: a.y, w: a.w, h: a.h });
                }
            }
        });
        let (p, t) = (proxy.clone(), tx.clone());
        tokio::spawn(async move {
            let Ok(mut s) = p.receive_focus_changed().await else { return };
            while let Some(sig) = s.next().await {
                if let Ok(a) = sig.args() {
                    let _ = t.send(ImeEvent::Focus { focused: a.focused, secure: a.password });
                }
            }
        });
        let (p, t) = (proxy.clone(), tx);
        tokio::spawn(async move {
            let Ok(mut s) = p.receive_reset().await else { return };
            while s.next().await.is_some() {
                let _ = t.send(ImeEvent::Reset);
            }
        });
        // fcitx5 restarted mid-session: the add-on is back, inactive.
        let (p, on) = (proxy.clone(), session.clone());
        tokio::spawn(async move {
            let Ok(mut s) = p.inner().receive_owner_changed().await else { return };
            while let Some(owner) = s.next().await {
                if owner.is_some()
                    && on.load(std::sync::atomic::Ordering::SeqCst)
                    && let Err(e) = p.activate().await
                {
                    eprintln!("remoted: ime: activating again after fcitx5 restarted: {e}");
                }
            }
        });
        Ok(ImeBridge { proxy, session })
    }

    pub async fn activate(&self) {
        self.session.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Err(e) = self.proxy.activate().await {
            eprintln!("remoted: ime: Activate: {e} (is the fcitx5 add-on installed?)");
        }
    }

    pub async fn deactivate(&self) {
        self.session.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Err(e) = self.proxy.deactivate().await {
            eprintln!("remoted: ime: Deactivate: {e}");
        }
    }

    pub async fn preedit(&self, text: &str, cursor: i32) -> bool {
        self.proxy.set_preedit(text, cursor).await.unwrap_or(false)
    }

    pub async fn commit(&self, text: &str) -> bool {
        self.proxy.commit(text).await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_caret_maps_password_purpose_to_secure_input() {
        assert_eq!(
            from_plugin_caret(true, 8, 259, 123, 0, 102),
            vec![ImeEvent::Focus { focused: true, secure: true }, ImeEvent::Caret { x: 259, y: 123, w: 0, h: 102 }]
        );
        // A PIN field with no rectangle: still secure input, no caret.
        assert_eq!(from_plugin_caret(false, 9, 0, 0, 0, 0), vec![ImeEvent::Focus { focused: true, secure: true }]);
    }
}
