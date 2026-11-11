# Configuration

All settings live in `~/.config/m3/` (or `$XDG_CONFIG_HOME/m3`). The Settings
app edits the same files; you can also edit them by hand — m3 keeps your
comments and formatting when it saves.

| File | Read by | Contents |
|---|---|---|
| `config.toml` | compositor | general behaviour, input devices, outputs, windows, idle, window rules |
| `keybindings.toml` | compositor | keyboard shortcuts |
| `workspaces.toml` | compositor | workspace count, names, layout |
| `theme.toml` | everything | colours, light/dark, fonts, shape, wallpaper |
| `panels.toml` | shell | panel position, modules, pinned apps, clock |
| `notifications.toml` | shell | popups, Do Not Disturb schedule, per-app settings |
| `lock.toml` | lock screen | background, clock, user picture, message, media and battery |
| `widgets.toml` | shell | desktop widgets: clock, greeting, media, countdown, storage, system, weather |

Every key is optional; missing keys use defaults. Apply changes with
`m3ctl reload` (the shell picks up `theme.toml`, `panels.toml` and
`notifications.toml` automatically).

## Safety rules

* An invalid file is **never overwritten**: m3 logs every problem with its
  location, and keeps using the previous (or default) settings.
* Unknown keys are reported and ignored.
* Every file has `schema_version`. Files from older m3 versions are upgraded
  in memory; the first save writes a backup (`config.toml.v1.bak`). Files
  written by a newer m3 are never modified.
* Saves are atomic (temporary file + rename) with owner-only permissions.

## config.toml

```toml
schema_version = 1

[general]
focus_follows_mouse = false
focus_new_windows = true
xwayland = true
cursor_theme = "default"
cursor_size = 24
hot_corner = true            # top-left corner opens the overview

[input.keyboard]
layout = "us,de"             # XKB layouts
variant = ""
options = "grp:alt_shift_toggle"
repeat_rate = 25             # per second
repeat_delay = 600           # ms
numlock = false

[input.pointer]
accel_speed = 0.0            # -1.0 … 1.0
accel_profile = "adaptive"   # or "flat"
natural_scroll = false
left_handed = false

[input.touchpad]
tap_to_click = true
natural_scroll = true
disable_while_typing = true
clickfinger = true
accel_speed = 0.0
scroll_factor = 1.0

[outputs.eDP-1]              # connector name, or "Make Model Serial"
scale = 1.25
mode = "2560x1600@120"
transform = "normal"         # 90, 180, 270, flipped, flipped_90, …
position = [0, 0]
primary = true

[outputs.HDMI-A-1]
enabled = false

[windows]
gaps = 8
master_ratio = 0.55          # tiling layout
snap_threshold = 16

[animations]
enabled = true
speed = 1.0                  # 2.0 = twice as fast

[idle]
lock_after = 300             # seconds, 0 = never
screen_off_after = 600       # turn displays off; any input wakes them
suspend_after = 0
lock_on_suspend = true

[night_light]
mode = "scheduled"           # off, always, scheduled
temperature = 4000           # kelvin, 1500 (warm) … 6500 (neutral)
from = "20:00"
until = "07:00"

[[window_rule]]
app_id = "pavucontrol"       # glob: * and ?
floating = true
size = [720, 520]

[[window_rule]]
title = "Picture-in-Picture"
floating = true
opacity = 0.95

[[window_rule]]
app_id = "org.mozilla.*"
workspace = 2
```

Window-rule fields: `app_id`, `title` (globs), `floating`, `workspace`,
`maximized`, `fullscreen`, `size`, `opacity`.

## keybindings.toml

```toml
schema_version = 1
inherit_defaults = true      # start from m3's defaults

[bindings]
"Super+Return" = { action = "spawn", command = ["foot"] }
"Super+E" = { action = "spawn", command = ["nautilus", "--new-window"] }
"Super+Q" = "disabled"
"Super+Ctrl+L" = { action = "set_layout", layout = "tiling" }
```

Modifiers: `Super`, `Ctrl`, `Alt`, `Shift`. Keys are XKB keysym names
(`Return`, `Print`, `XF86AudioRaiseVolume`, letters, digits). Commands run
directly — never through a shell. Actions: `spawn`, `spawn_terminal`,
`close_window`, `toggle_maximize`, `toggle_fullscreen`, `minimize`,
`toggle_floating`, `snap` (`direction`), `switch_workspace` (`index`),
`move_to_workspace`, `next_workspace`, `previous_workspace`, `cycle_layout`,
`set_layout`, `cycle_windows` (`reverse`), `focus_next`, `focus_previous`,
`focus_direction`, `toggle_overview`, `toggle_launcher`,
`toggle_quick_settings`, `toggle_notifications`, `lock`, `screenshot`, `screenshot_region`,
`volume_up/down/mute`, `mic_mute`, `brightness_up/down`,
`media_play_pause/next/previous`, `displays_off`, `reload_config`, `restart_shell`, `quit`.
`Ctrl+Alt+F1…F12` always switch virtual terminals.

## workspaces.toml

```toml
count = 4
names = ["Web", "Code", "Chat"]
default_layout = "floating"  # tiling, monocle
wrap_around = false
```

## theme.toml

```toml
mode = "auto"                # light, dark, auto
dark_from = "19:00"
dark_until = "07:00"
source = "wallpaper"         # wallpaper, seed, preset
seed_color = "#8c4a5c"
preset = "wine"              # violet, ocean, teal, forest, sunflower, coral, rose, slate
variant = "content"          # tonal_spot, vibrant, expressive, fidelity, neutral, monochrome, rainbow, fruit_salad
contrast = 0.0               # -1 … 1
accent_intensity = 1.0       # 0 (grey) … 2
surface_tint = 1.0           # colour in surfaces, 0 … 3
corner_scale = 1.0           # 0 = square
font_family = "Google Sans Flex"
monospace_font_family = "JetBrains Mono"
text_scale = 1.0
icon_theme = "Adwaita"
transparency = true
reduced_motion = false
high_contrast = false

[wallpaper]
path = "/home/me/Pictures/mountains.jpg"   # empty = m3's built-in wallpaper
fit = "fill"                 # fit, center, stretch
slideshow_dir = ""           # a folder enables the slideshow
slideshow_minutes = 30
slideshow_shuffle = true
transition_ms = 600

[wallpaper.outputs]
HDMI-A-1 = "/home/me/Pictures/second.png"
```

## panels.toml

```toml
position = "bottom"          # top, left, right
visibility = "always"        # autohide, intelligent
size = 48
floating = true
style = "islands"            # each module its own pill; or "bar"
all_outputs = true
start = ["power", "workspaces"]
center = ["launcher", "search", "tasks"]
end = ["tray", "status", "notifications", "clock"]
pinned = ["org.mozilla.firefox.desktop", "foot.desktop"]
clock_format = "h24"         # h12
clock_seconds = false
clock_date = true
battery_percentage = true
```

## notifications.toml

```toml
popups = true
dnd_schedule = true
dnd_from = "22:00"
dnd_until = "07:00"

[apps.Thunderbird]
enabled = true
popups = false               # history only
```

## lock.toml

Read each time the screen locks (Settings → Lock screen has a preview).
Appearance only: nothing here affects how you unlock.

```toml
background = "wallpaper"     # wallpaper, image, color
image = "/home/me/Pictures/lock.jpg"   # for background = "image"
blur = 24                    # 0 … 64 px
dim = 0.4                    # 0 … 0.9

clock_format = "h24"         # h12
clock_seconds = false
clock_size = "large"         # small, medium
clock_style = "expressive"   # light, classic
clock_position = "center"    # top, left
show_date = true
date_format = "long"         # short

show_user = true
avatar_shape = "cookie12"    # circle, square, cookie4, cookie9, clover4, clover8, sunny, flower, burst, pentagon
message = "If found, please call +1 555 0100"   # plain text, at most 200 characters

show_media = true            # now playing with play/pause and next
show_battery = true
```

The user picture is `~/.face` when it exists, otherwise your initial.

## widgets.toml

Desktop widgets above the wallpaper. Weather is off until you set a place:
it asks [Open-Meteo](https://open-meteo.com) (no account) for the forecast.

```toml
enabled = true
clock = true                 # big clock on the left
greeting = true              # "Good afternoon" with day, month and your name
name = ""                    # empty = your account's real name
side = "right"               # widget column: right or left
day = true
shortcuts = true
shortcut_apps = ["org.m3.Browser.desktop", "org.m3.Files.desktop", "org.m3.Music.desktop"]
media = true                 # now playing (any MPRIS player)
storage = true
system = true                # RAM and CPU

[countdown]
enabled = true
title = "Summer holiday"     # empty hides the countdown
date = "2027-07-01"

# Written when you move or resize widgets (right-click the desktop → Edit
# widgets): top-left corner as a fraction of the desktop, and a size factor.
[layout.clock]
x = 0.06
y = 0.3
scale = 1.2

[weather]
enabled = true
place = "Warsaw"
latitude = 52.23
longitude = 21.01
unit = "celsius"             # or fahrenheit
```
