// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The portal Clipboard: text, HTML and PNG both
//! ways while a portal session runs. The computer's changes come as
//! SelectionOwnerChanged and are read with SelectionRead; the device's go out with
//! SetSelection and are written on SelectionTransfer (`portal.rs`). A secret is marked
//! `x-kde-passwordManagerHint` both ways.

use std::sync::Arc;

use super::portal::Portal;
use crate::backend::{ClipEntry, ClipSink, Clipboard};

pub struct PortalClipboard {
    portal: Arc<Portal>,
}

impl PortalClipboard {
    pub fn start(portal: Arc<Portal>, on_change: ClipSink) -> Arc<dyn Clipboard> {
        portal.set_clip_sink(on_change);
        Arc::new(PortalClipboard { portal })
    }
}

impl Clipboard for PortalClipboard {
    fn get(&self) -> Option<ClipEntry> {
        self.portal.clip_current()
    }
    fn set(&self, entry: ClipEntry) {
        self.portal.clip_offer(entry);
    }
}
