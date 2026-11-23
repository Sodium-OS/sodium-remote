// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Functions the Remote engine exports to other fcitx5 addons and to its own test.
// remoted itself uses the D-Bus interface.
#pragma once

#include <string>
#include <fcitx/addoninstance.h>

FCITX_ADDON_DECLARE_FUNCTION(RemoteIme, activateSession, void());
FCITX_ADDON_DECLARE_FUNCTION(RemoteIme, deactivateSession, void());
FCITX_ADDON_DECLARE_FUNCTION(RemoteIme, setPreedit, bool(const std::string &, int));
FCITX_ADDON_DECLARE_FUNCTION(RemoteIme, commit, bool(const std::string &));
