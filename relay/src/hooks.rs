// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Quota and account hooks: off by default (`NoHooks` allows
//! everything). A hosted service plugs in its own to refuse with `quota_exceeded` or
//! `payment_required`. The open-source server has no billing.
//!
//! Hooks run while the hub's state is locked: answer from memory, quickly, and
//! never call back into the hub.

use std::net::IpAddr;

use serde_json::Value;

use crate::proto::{DeviceId, Role};

/// Why a hook refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    QuotaExceeded,
    PaymentRequired,
    /// The device must be signed in to an account first.
    AccountRequired,
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
    /// A `connect` succeeded. `wish` is the `node` the client asked for, if any. Returning
    /// `Some((for_client, for_host))` adds `"routes"` to the client's `connect` result and
    /// to the host's `incoming` event.
    fn route_offer(
        &self,
        _session: &str,
        _client: &DeviceId,
        _host: &DeviceId,
        _client_ip: IpAddr,
        _host_ip: IpAddr,
        _wish: Option<u16>,
    ) -> Option<(Value, Value)> {
        None
    }
    /// A session's client or host sends `routes`. `Some` is the reply's `"routes"`.
    fn route_refresh(&self, _session: &str, _device: &DeviceId) -> Option<Value> {
        None
    }
    /// Extra data for this device's `auth_ok`, sent under the key `"service"`.
    fn auth_extra(&self, _device: &DeviceId, _role: Role, _account: Option<&str>) -> Option<Value> {
        None
    }
    /// A session ended, for whatever reason (closed, disconnected, revoked, …).
    fn session_closed(&self, _session: &str) {}
}

/// No quotas: what the open-source server runs with.
pub struct NoHooks;

impl Hooks for NoHooks {}
