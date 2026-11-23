// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! A plain HTTP(S) GET through WinHTTP (system proxy settings, system certificate store), for the
//! one thing Sodium Remote downloads: Cisco's OpenH264 DLL (`softvideo`). No other crate, no
//! other network code; the file is checked against a pinned SHA-256 afterwards.

use crate::softvideo::Fetch;
use windows::Win32::Networking::WinHttp::*;
use windows::core::PCWSTR;

pub struct WinHttp;

struct Handle(*mut std::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `scheme://host[:port]/path` to (https, host, port, path).
fn split(url: &str) -> Result<(bool, &str, u16, &str), String> {
    let (https, rest) = match url.split_once("://") {
        Some(("https", r)) => (true, r),
        Some(("http", r)) => (false, r),
        _ => return Err(format!("not an http(s) URL: {url}")),
    };
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("bad port in {url}"))?),
        None => (authority, if https { 443 } else { 80 }),
    };
    Ok((https, host, port, path))
}

impl Fetch for WinHttp {
    fn get(&self, url: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
        let (https, host, port, path) = split(url)?;
        let err = |what: &str, e: windows::core::Error| format!("{what}: {e}");
        unsafe {
            let agent = wide("SodiumRemote");
            let session = Handle(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
            if session.0.is_null() {
                return Err(err("WinHttpOpen", windows::core::Error::from_thread()));
            }
            // Connect 15 s, send and receive 30 s each: a stuck network ends in `failed`, not a hang.
            WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000).map_err(|e| err("WinHttpSetTimeouts", e))?;
            let h = wide(host);
            let conn = Handle(WinHttpConnect(session.0, PCWSTR(h.as_ptr()), port, 0));
            if conn.0.is_null() {
                return Err(err("WinHttpConnect", windows::core::Error::from_thread()));
            }
            let (verb, p) = (wide("GET"), wide(path));
            let flags = if https { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
            let req = Handle(WinHttpOpenRequest(conn.0, PCWSTR(verb.as_ptr()), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
            if req.0.is_null() {
                return Err(err("WinHttpOpenRequest", windows::core::Error::from_thread()));
            }
            WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|e| err("WinHttpSendRequest", e))?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| err("WinHttpReceiveResponse", e))?;

            let number = |query: u32| -> Option<u32> {
                let (mut v, mut n) = (0u32, 4u32);
                WinHttpQueryHeaders(req.0, query | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut v as *mut u32 as *mut _), &mut n, std::ptr::null_mut()).ok().map(|_| v)
            };
            match number(WINHTTP_QUERY_STATUS_CODE) {
                Some(200) => {}
                other => return Err(format!("{url}: HTTP status {other:?}")),
            }
            let total = number(WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from);
            let mut body = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut got = 0u32;
                WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut got).map_err(|e| err("WinHttpReadData", e))?;
                if got == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..got as usize]);
                if !progress(body.len() as u64, total) {
                    return Err("stopped".into());
                }
            }
            Ok(body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn urls_split() {
        assert_eq!(split("http://ciscobinary.openh264.org/a.bz2").unwrap(), (false, "ciscobinary.openh264.org", 80, "/a.bz2"));
        assert_eq!(split("https://h:8443/x/y").unwrap(), (true, "h", 8443, "/x/y"));
        assert!(split("ftp://h/x").is_err());
    }
}
