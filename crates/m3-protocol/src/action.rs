use serde::{Deserialize, Serialize};

/// Everything a keybinding (or `m3ctl`) can ask the compositor to do.
///
/// Actions are plain data. `Spawn` executes an argv directly (never through a
/// shell) and is only accepted from the user's own keybinding file or from an
/// authenticated IPC peer (same UID).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Spawn {
        command: Vec<String>,
    },
    /// Spawn the user's terminal: `$TERMINAL`, then `xdg-terminal-exec`, then
    /// the first well-known terminal emulator found in `$PATH`.
    SpawnTerminal,
    CloseWindow,
    /// Focus a window by id, switching workspace and un-minimizing it if needed.
    FocusWindow {
        id: u64,
    },
    CloseWindowById {
        id: u64,
    },
    FocusNext,
    FocusPrevious,
    FocusDirection {
        direction: Direction,
    },
    ToggleMaximize,
    ToggleFullscreen,
    Minimize,
    ToggleFloating,
    /// Snap the focused window to half / quarter of the output.
    Snap {
        direction: Direction,
    },
    /// 1-based workspace index.
    SwitchWorkspace {
        index: usize,
    },
    NextWorkspace,
    PreviousWorkspace,
    MoveToWorkspace {
        index: usize,
    },
    MoveWindowToWorkspace {
        id: u64,
        index: usize,
    },
    /// Cycle the active workspace's layout (floating → tiling → monocle).
    CycleLayout,
    SetLayout {
        layout: String,
    },
    /// Begin / step an Alt+Tab style window cycle.
    CycleWindows {
        reverse: bool,
    },
    ToggleOverview,
    ToggleLauncher,
    ToggleQuickSettings,
    ToggleNotifications,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
    MediaPlayPause,
    MediaNext,
    MediaPrevious,
    Lock,
    /// Power the displays off now; any input turns them back on.
    DisplaysOff,
    Screenshot,
    /// Select a region with the pointer, then capture it (shell UI).
    ScreenshotRegion,
    ReloadConfig,
    /// Restart the desktop shell (panel, launcher, notifications).
    RestartShell,
    /// Switch virtual terminal (DRM backend only).
    SwitchVt {
        vt: i32,
    },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Action {
    /// Actions handled by the desktop shell rather than the compositor.
    pub fn is_shell_action(&self) -> bool {
        matches!(
            self,
            Action::ToggleOverview
                | Action::ToggleLauncher
                | Action::ToggleQuickSettings
                | Action::ToggleNotifications
                | Action::VolumeUp
                | Action::VolumeDown
                | Action::VolumeMute
                | Action::MicMute
                | Action::BrightnessUp
                | Action::BrightnessDown
                | Action::MediaPlayPause
                | Action::MediaNext
                | Action::MediaPrevious
                | Action::ScreenshotRegion
        )
    }
}
