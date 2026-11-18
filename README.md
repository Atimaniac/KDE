# Delfin — a KDE-style desktop for Windows (Rust)

A Dolphin-style file manager **and** a full desktop shell that replaces `explorer.exe`:
bottom panel like KDE Plasma, application launcher, desktop with your wallpaper.
Breeze Dark / Breeze Light look, written in Rust with egui.

## Desktop shell (no Windows Explorer)

`delfin.exe --shell` runs:

- **Panel** at the bottom: launcher button, pinned file manager + terminal, task list with real
  window icons (click = switch / minimize, middle click = close), a Wi-Fi / volume button showing the real signal and volume,
  clock, *show desktop* strip. Maximized windows don't cover it.
- **Launcher** (Kickoff-like): all Start Menu apps and Store apps with search. Enter runs the first match, or the
  typed text as a command (like the Run dialog). Lock / Log Out / Restart / Shut Down (click twice).
- **Quick Settings** (click the Wi-Fi / volume button; right-click opens Sound settings): volume and
  mute, monitor brightness (DDC/CI monitors and laptop screens), play / pause / next / previous,
  Wi-Fi on/off and a list of networks to connect to (with a password) or disconnect, battery level
  on laptops, shortcuts to Windows Settings.
- **Desktop**: your Windows wallpaper, Desktop folder icons (double-click to open), right-click menu.
- **System tray** (notification area) that works like Explorer's: programs such as Steam,
  Discord or OneDrive show their icons, can hide to the tray when minimized, and get left / right /
  double / middle clicks and their context menus. Tooltips, balloon notifications and a **^**
  overflow for icons beyond 8. Already-running programs re-add their icons when the shell starts.
- **Minimize to tray** (optional, launcher → *Minimize to tray*): any minimized window is hidden
  into the tray (blue underline); click it to bring it back.
- **Startup apps** from the registry *Run* keys and Startup folders (disabled entries in Task
  Manager are respected).
- **Shortcuts**: `Win+E` file manager, `Win+D` show desktop, `Win+R` / `Alt+F1` launcher.
- Folders opened anywhere in Windows open in Delfin.

### Try it without installing

```
taskkill /f /im explorer.exe
target\release\delfin.exe --shell
```
To get Explorer back: launcher → *Back to Explorer*, or Ctrl+Alt+Del → Task Manager →
*Run new task* → `explorer.exe`.

### Install as your shell

Delfin menu (☰) → *System* → **Use Delfin as Windows shell…**, or `delfin.exe --install-shell`.
Then sign out and back in. It's per user (HKCU, no admin). The exe is copied to
`%LOCALAPPDATA%\Programs\Delfin\delfin.exe`.

**Go back:** launcher → *Back to Explorer* (click twice), or `delfin.exe --uninstall-shell`.
Emergency (black screen): Ctrl+Alt+Del → Task Manager → *Run new task* → `explorer.exe`,
then run `%LOCALAPPDATA%\Programs\Delfin\delfin.exe --uninstall-shell`.

### Not there yet

- Windows 10/11 *toast* notifications (WinRT apps) aren't shown; classic tray balloons are.
- Volume/network are shortcuts to Settings, not flyouts.
- Apps that call `explorer.exe /select,…` ("Show in folder") still open an Explorer window.
- Primary monitor only.

## File manager

## Build & run

1. Install Rust: https://rustup.rs (choose the MSVC toolchain on Windows)
2. In this folder:

```
cargo run --release
```

The finished program is `target\release\delfin.exe`. You can pin it to the taskbar.
You can also start it in a folder: `delfin.exe D:\Projects`

Tip: install the **Noto Sans** font (KDE's font). Delfin uses it if it finds it. Otherwise it uses Segoe UI.

## Features (Dolphin-style)

- Toolbar with Back / Forward / Up / Home, a clickable **breadcrumb** location bar (click the empty area or Ctrl+L to type a path)
- **Places** panel (Home, Desktop, Documents, Downloads, Music, Pictures, Videos, your own places) and **Devices** with capacity bars
- **Icons / Compact / Details** views; zoom slider + Ctrl+mouse wheel
- **Tabs** (Ctrl+T, middle-click a folder) and **split view** (F3)
- **Information panel** (F11)
- Filter-as-you-type (Ctrl+F)
- Copy / Cut / Paste (runs in the background), Rename (F2), Move to Trash (Del), Delete permanently (Shift+Del), New Folder (Ctrl+Shift+N)
- Open Terminal Here (F4) → Windows Terminal, otherwise PowerShell
- Hidden files toggle (Ctrl+H), sort by name/size/date/type, folders first
- Mouse back/forward buttons work
- Settings are saved in `%APPDATA%\delfin\config.ini`

## Keyboard shortcuts

| Key | Action |
|---|---|
| Alt+Left / Alt+Right / Alt+Up / Backspace | Back / Forward / Up |
| Alt+Home | Home |
| Enter | Open |
| F2 | Rename |
| F3 | Split view |
| F4 | Terminal |
| F5 | Refresh |
| F9 / F11 | Places panel / Information panel |
| Ctrl+1 / 2 / 3 | Icons / Compact / Details |
| Ctrl+T / Ctrl+W / Ctrl+PgUp/PgDn | New tab / Close tab / Switch tab |
| Ctrl+L / Ctrl+F | Edit location / Filter |
| Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V | Select all, copy, cut, paste |
| Del / Shift+Del | Trash / Delete permanently |

## Project layout

- `src/app.rs`: window, panels, views, actions, shortcuts
- `src/theme.rs`: Breeze colours and fonts
- `src/icons.rs`: vector-painted Breeze-style icons
- `src/fsops.rs`: listing, sorting, copy/move/delete
- `src/platform.rs`: Windows drives, free space, terminal
- `src/config.rs`: settings file
