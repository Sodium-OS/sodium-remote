#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""The guest's witness for the generic Linux host's end to end (tests/linux/e2e.sh).

A maximised GTK 4 window with a text view, in the VM's GNOME or Plasma session. It
records what the host injected and works the clipboard on request, all in ~/sr-test/:
  text.txt     the text view's contents after every change (typing, IME commits)
  clicks.txt   one "x y button" line per press, window coordinates (logical pixels)
  cmd          written by the test: "read" puts the clipboard's text in clip-in.txt;
               "set TEXT" puts TEXT on the clipboard. Removed once done.
GTK reads and writes the Wayland clipboard only while focused, which this window is.
"""
import os

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

D = os.path.expanduser("~/sr-test")
os.makedirs(D, exist_ok=True)


def write(name, text, mode="w"):
    with open(os.path.join(D, name), mode, encoding="utf-8") as f:
        f.write(text)


def on_activate(app):
    win = Gtk.ApplicationWindow(application=app, title="sr-testapp")
    view = Gtk.TextView()
    view.set_monospace(True)
    buf = view.get_buffer()
    buf.connect("changed", lambda b: write("text.txt", b.get_text(b.get_start_iter(), b.get_end_iter(), False)))
    click = Gtk.GestureClick(button=0)
    click.connect("pressed", lambda g, n, x, y: write("clicks.txt", f"{x:.0f} {y:.0f} {g.get_current_button()}\n", "a"))
    view.add_controller(click)
    win.set_child(view)
    mon = os.environ.get("SR_MONITOR")  # "1": fill the second monitor (several-monitor runs); else maximised
    if mon is not None:
        win.fullscreen_on_monitor(Gdk.Display.get_default().get_monitors().get_item(int(mon)))
    else:
        win.maximize()
    win.present()
    view.grab_focus()
    clip = Gdk.Display.get_default().get_clipboard()

    def poll():
        p = os.path.join(D, "cmd")
        if os.path.exists(p):
            with open(p, encoding="utf-8") as f:
                cmd = f.read()
            os.remove(p)
            if cmd.startswith("read"):
                def done(c, res):
                    try:
                        write("clip-in.txt", c.read_text_finish(res) or "")
                    except GLib.Error as e:
                        write("clip-in.txt", f"ERROR {e.message}")
                clip.read_text_async(None, done)
            elif cmd.startswith("set "):
                clip.set(cmd[4:])
        return True

    GLib.timeout_add(200, poll)


app = Gtk.Application(application_id="org.sodiumos.RemoteTestWitness")
app.connect("activate", on_activate)
app.run(None)
