// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Whether this computer's screen is locked: logind's `LockedHint` on this process's session, read from the
//! system bus. GNOME ends the portal session when it locks (and refuses a new one), so the host asks this before it
//! calls the end "stopped at the computer". KDE keeps the session and needs no answer.
//!
//! GNOME Shell 51 in the test VM (an autologin session whose shell runs as a user service) never set the hint, so
//! GNOME's own `org.gnome.ScreenSaver.GetActive` on the session bus is read too: the screen is locked when either says so.

use std::time::Duration;

/// Where the hint is read again when the first read says "unlocked": the desktop sets the hint a moment after
/// the portal closes.
pub const REREAD_AFTER: Duration = Duration::from_millis(300);

/// The first read, and the second only when the first says unlocked. A read that fails counts as unlocked.
pub async fn locked_after_reread<F, Fut>(mut read: F, wait: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<bool>>,
{
    if read().await == Some(true) {
        return true;
    }
    tokio::time::sleep(wait).await;
    read().await == Some(true)
}

/// The decision with the real hint (and GNOME's screen shield).
pub async fn is_locked() -> bool {
    locked_after_reread(read_locked, REREAD_AFTER).await
}

/// Locked when logind's hint or GNOME's screen shield says so; unknown only when neither could be read.
async fn read_locked() -> Option<bool> {
    let wait = Duration::from_secs(2);
    let hint = tokio::time::timeout(wait, read_hint()).await.ok().flatten();
    if hint == Some(true) {
        return Some(true);
    }
    let shield = tokio::time::timeout(wait, read_shield()).await.ok().flatten();
    match (hint, shield) {
        (None, None) => None,
        (a, b) => Some(a == Some(true) || b == Some(true)),
    }
}

/// GNOME's `org.gnome.ScreenSaver.GetActive`: true while the lock screen (or its blank screen) is up. None without GNOME's shell.
async fn read_shield() -> Option<bool> {
    let conn = zbus::Connection::session().await.ok()?;
    let saver = zbus::Proxy::new(&conn, "org.gnome.ScreenSaver", "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver").await.ok()?;
    saver.call::<_, _, bool>("GetActive", &()).await.ok()
}

/// This process's logind session: `XDG_SESSION_ID`, else the session of the process, else the user's graphical session (a
/// program started from a user service is in no session scope of its own).
async fn read_hint() -> Option<bool> {
    use zbus::OwnedObjectPath;
    let conn = zbus::Connection::system().await.ok()?;
    let manager = zbus::Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await.ok()?;
    let by_env = match std::env::var("XDG_SESSION_ID").ok().filter(|s| !s.is_empty()) {
        Some(id) => manager.call::<_, _, OwnedObjectPath>("GetSession", &(id,)).await.ok(),
        None => None,
    };
    let by_pid = match by_env {
        Some(p) => Some(p),
        None => manager.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),)).await.ok(),
    };
    let path = match by_pid {
        Some(p) => p,
        None => {
            let user: OwnedObjectPath = manager.call("GetUser", &(rustix::process::getuid().as_raw(),)).await.ok()?;
            let user = zbus::Proxy::new(&conn, "org.freedesktop.login1", user, "org.freedesktop.login1.User").await.ok()?;
            let (_, display): (String, OwnedObjectPath) = user.get_property("Display").await.ok()?;
            display
        }
    };
    if path.as_str() == "/" {
        return None;
    }
    let session = zbus::Proxy::new(&conn, "org.freedesktop.login1", path, "org.freedesktop.login1.Session").await.ok()?;
    session.get_property::<bool>("LockedHint").await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    async fn decide(answers: Vec<Option<bool>>) -> (bool, u32) {
        let n = AtomicU32::new(0);
        let r = locked_after_reread(
            || {
                let i = n.fetch_add(1, Ordering::SeqCst) as usize;
                let a = answers[i.min(answers.len() - 1)];
                async move { a }
            },
            Duration::from_millis(1),
        )
        .await;
        (r, n.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn locked_is_final_and_unlocked_is_read_once_more() {
        assert_eq!(decide(vec![Some(true)]).await, (true, 1));
        assert_eq!(decide(vec![Some(false), Some(true)]).await, (true, 2));
        assert_eq!(decide(vec![Some(false)]).await, (false, 2));
        assert_eq!(decide(vec![None]).await, (false, 2));
    }
}
