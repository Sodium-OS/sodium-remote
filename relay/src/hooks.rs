// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Quota and account hooks: off by default (`NoHooks` allows
//! everything). A hosted service plugs in its own to refuse with `quota_exceeded` or
//! `payment_required`. The open-source server has no billing.
//!
//! Hooks run while the hub's state is locked: answer from memory, quickly.

use crate::proto::{DeviceId, Role};

/// Why a hook refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    QuotaExceeded,
    PaymentRequired,
}

pub trait Hooks: Send + Sync {
    /// A new device is registering. `account` comes from its enrolment token.
    fn register(&self, _device: &DeviceId, _role: Role, _account: Option<&str>) -> Result<(), Denial> {
        Ok(())
    }
    /// A device asks for TURN credentials (the relay is the costly part).
    fn turn_credentials(&self, _device: &DeviceId, _account: Option<&str>) -> Result<(), Denial> {
        Ok(())
    }
    /// A client asks for a host (a signalling session).
    fn connect(&self, _client: &DeviceId, _host: &DeviceId, _account: Option<&str>) -> Result<(), Denial> {
        Ok(())
    }
}

/// No quotas: what the open-source server runs with.
pub struct NoHooks;

impl Hooks for NoHooks {}
