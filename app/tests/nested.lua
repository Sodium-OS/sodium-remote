-- Nested Hyprland for the Remote app's end-to-end run and screenshots (tests/e2e.sh): the client screen. Scale 1,
-- the board's ground, window glass blurred, every Remote window floating. Starts no daemons.
hl.monitor({
    output   = "",
    mode     = "preferred",
    position = "auto",
    scale    = "1",
})

hl.config({
    misc = {
        disable_hyprland_logo    = true,
        disable_splash_rendering = true,
        background_color         = "rgba(0e1018ff)",
    },
    general    = { border_size = 0, gaps_in = 0, gaps_out = 0 },
    decoration = {
        rounding = 16,
        rounding_power = 4,
        shadow   = { enabled = true, range = 40, render_power = 3, color = "rgba(00000080)" },
        blur     = { enabled = true, size = 8, passes = 4 },
    },
    animations = { enabled = false },
})

hl.window_rule({ name = "remote-float", match = { class = "^org\\.sodiumos\\.Remote$" }, float = true })
