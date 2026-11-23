-- Nested Hyprland for the end-to-end host: spikes/nested/minimal.lua plus the sodiumwm
-- plugin built from this tree (REMOTE_PLUGIN, compositor/plugin/build.sh --test),
-- whose capture socket is SODIUMWM_REMOTE_SOCKET (never the live session's). The
-- nested window's own output (WAYLAND-1) is the desk; remoted creates REMOTE-1
-- and mirrors the desk to it, as on the real desktop. No daemons.
hl.monitor({
    output   = "",
    mode     = os.getenv("DESK_MODE") or "1920x1080",
    position = "0x0",
    scale    = "1",
})

hl.config({
    misc = {
        disable_hyprland_logo = true,
        disable_splash_rendering = true,
        -- "started without start-hyprland": its notification animates on REMOTE-1
        -- and would cost idle bandwidth that a real desktop doesn't have.
        disable_watchdog_warning = true,
    },
    input = {
        kb_layout = "us",
        follow_mouse = 2,
        sensitivity = 0,
    },
})

-- The live session's pointer and keyboard reach a nested instance through the
-- wayland backend when the live cursor crosses its window: only the remote
-- client's injected devices may act here.
hl.device({ name = "wl_pointer", enabled = false })
hl.device({ name = "wl_keyboard", enabled = false })

-- run.sh --text: the dense page floats where the script puts it, beside the click marker.
hl.window_rule({ name = "e2e-page", match = { title = "^remote-page$" }, float = true })

local plugin = os.getenv("REMOTE_PLUGIN")
if plugin and plugin ~= "" then
    hl.plugin.load(plugin)
end
