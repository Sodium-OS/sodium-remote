// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Where a caret is on screen: only the compositor knows where surfaces are, so the
// engine asks Hyprland's IPC, where the sodiumwm plugin's remote hook answers
// (spikes/remote-ime/caret-plugin is the prototype; remote/docs/reference/ime-and-dnd.md).
#pragma once

#include <optional>
#include <string>

struct OutputCaret {
    std::string output;
    int x = 0, y = 0, w = 0, h = 0;
    bool operator==(const OutputCaret &) const = default;
};

// The focused text-input-v3/v1 field's caret (apps that talk text-input to the
// compositor; fcitx5 never sees their caret).
std::optional<OutputCaret> textInputCaret();

// A caret relative to the keyboard-focused surface, in that surface's pixels at `scale`
// (what fcitx5's D-Bus frontend gets from Qt's and GTK's fcitx modules on Wayland).
std::optional<OutputCaret> mapSurfaceCaret(int x, int y, int w, int h, double scale);
