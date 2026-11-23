// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The main thread. NSScreen is main-thread-only in AppKit, and NSCursor and NSWorkspace
//! are kept current by the main run loop, so the backend reads them there: `on_main`
//! queues the work on the main dispatch queue and waits a bounded time, never forever
//! (the app's main thread may itself be waiting on us, as when the pane switches the
//! host off). In Remote.app AppKit runs the main loop; the program runs
//! `run_main_loop` on its main thread and the session on another.

use std::sync::mpsc;
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Runs `f` on the main thread and returns its result, or None if the main thread
/// didn't get to it within `timeout`. On the main thread already, it runs at once.
pub fn on_main<R, F>(timeout: Duration, f: F) -> Option<R>
where
    R: Send + 'static,
    F: FnOnce(MainThreadMarker) -> R + Send + 'static,
{
    if let Some(mtm) = MainThreadMarker::new() {
        return Some(autoreleasepool(|_| f(mtm)));
    }
    let (tx, rx) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
        let r = autoreleasepool(|_| f(mtm));
        let _ = tx.send(r);
    });
    rx.recv_timeout(timeout).ok()
}

/// The program's main thread: an AppKit application that never shows a window or a
/// Dock icon, running the main run loop until the process exits.
pub fn run_main_loop() -> ! {
    let mtm = MainThreadMarker::new().expect("run_main_loop on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    loop {
        autoreleasepool(|_| {
            objc2_core_foundation::CFRunLoop::run_in_mode(
                unsafe { objc2_core_foundation::kCFRunLoopDefaultMode },
                1.0,
                false,
            );
        });
    }
}
