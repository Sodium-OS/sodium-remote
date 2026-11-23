-- Nested Hyprland for run.sh: sodiumwm (Lua core and plugin) from this checkout,
-- and a headless DESK standing in for DP-2 (3840 x 2160 at 1.5: 2560 x 1440 points).
-- The host window's own output is switched off once DESK exists, so the live
-- session re-tiling the nested window never resizes anything mid-test.
-- There is no bar: nothing reserves the top edge, on DESK or on REMOTE-1.
local here = debug.getinfo(1, "S").source:match("^@(.*/)") or "./"
local repo = here .. "../../../../"
package.path = repo .. "compositor/lua/?.lua;" .. repo .. "compositor/lua/?/init.lua;" .. package.path

hl.monitor({ output = "DESK", mode = "3840x2160@60", position = "0x0", scale = "1.5" })
local headless = false
for _, m in ipairs(hl.get_monitors() or {}) do
    if m.name == "DESK" then headless = true end
end
if headless then
    hl.monitor({ output = "WAYLAND-1", disabled = true })
else
    hl.monitor({ output = "WAYLAND-1", mode = "preferred", position = "auto", scale = "1" })
end

local so = os.getenv("SODIUMWM_PLUGIN")
if so and so ~= "" then pcall(hl.plugin.load, so) end

pcall(require, "look")
hl.config({
    general = { gaps_in = 5, gaps_out = 14, border_size = 4, snap = { enabled = false } },
    misc = { disable_hyprland_logo = true, disable_splash_rendering = true, disable_watchdog_warning = true },
    debug = { disable_logs = false },
})

local tmp = os.getenv("SODIUMWM_TEST_DIR") or "/tmp/sodiumwm-nested"
require("sodiumwm").setup({
    state_dir = os.getenv("SODIUMWM_STATE_DIR") or (tmp .. "/state"),
    runtime_dir = os.getenv("SODIUMWM_RUNTIME_DIR") or (tmp .. "/run"),
    shell = repo .. "spikes/3-sodiumwm/fake-qs.sh",
    keys = false,
})
