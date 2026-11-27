# Delfin — a KDE-style desktop for Windows (Rust)

A Dolphin-style file manager **and** a full desktop shell that replaces `explorer.exe`:
bottom panel like KDE Plasma, application launcher, desktop with your wallpaper.
Breeze Dark / Breeze Light look, written in Rust with egui.

## Desktop shell (no Windows Explorer)

`delfin.exe --shell` runs:

- **Panel** at the bottom: launcher button, pinned file manager + terminal, task list with real
  window icons (click = switch / minimize, middle click = close), separate network and volume
  buttons showing the real Wi-Fi signal and volume (mouse wheel over the speaker changes the
  volume), and a clock that opens a month calendar. Maximized windows don't cover it.
- **Launcher**: a search box; the list stays empty until you type or pick a category on the left
  (Office, Graphics, Internet, Multimedia, System Tools, Games, Recently Used, All Applications).
  Start Menu apps and Store apps, with descriptions. Enter runs the selected app, or the typed text
  as a command (like the Run dialog). On the right: This PC, Documents, Downloads, Settings, Task
  Manager, Run, **Delfin** settings, and Lock / Shut Down / Restart / Log Out (click twice).
- **Delfin settings** (launcher → *Delfin*): English or Polish, updates from the latest GitHub
  release (checked against the SHA-256 in its notes, installed in place, then a restart), and the
  shell options *Minimize to tray* and *Back to Explorer*.
- **Flyouts**: the volume button opens volume and mute, monitor brightness (DDC/CI monitors and
  laptop screens), play / pause / next / previous and the battery level on laptops; the network
  button opens Wi-Fi on/off and a list of networks to connect to (with a password) or disconnect.
  Right-click either one for the matching Windows Settings page.
- **Desktop**: your Windows wallpaper, Desktop folder icons (double-click to open). Right-click an icon for
  Open With…, Run as administrator, Cut / Copy, Paste Into Folder, Copy Location, archives, Create Shortcut,
  Rename (F2), Trash and Properties; right-click the wallpaper for a new folder or text document and Paste.
  Ctrl+C / Ctrl+X / Ctrl+V work there too.
- **System tray** (notification area) (takes over as soon as Explorer is closed, also while the shell runs) that works like Explorer's: programs such as Steam,
  Discord or OneDrive show their icons, can hide to the tray when minimized, and get left / right /
  double / middle clicks and their context menus. Tooltips, balloon notifications and a **^**
  overflow for icons beyond 8. Already-running programs re-add their icons when the shell starts.
- **Minimize to tray** (optional, Delfin settings → *Minimize to tray*): any minimized window is hidden
  into the tray (blue underline); click it to bring it back.
- **Startup apps** from the registry *Run* keys and Startup folders (disabled entries in Task
  Manager are respected).
- **Task Manager** (`delfin.exe --taskmgr`, Ctrl+Shift+Esc): processes grouped into apps and background
  processes with CPU, memory, disk and GPU use (end task, efficiency mode, priority, open location),
  performance graphs (CPU per core and speed, memory, disks, network, GPU with NVIDIA temperature),
  start-up apps (enable / disable), details and services (start / stop). With *Replace Windows Task
  Manager* (installer, Delfin settings or `delfin.exe --replace-taskmgr on|off`, needs administrator
  rights once) every start of taskmgr.exe, also from Ctrl+Alt+Del, opens it; this uses the same
  "Image File Execution Options" entry as Process Explorer's "Replace Task Manager".
- **Device Manager** (`delfin.exe --devmgr`, launcher → *Device Manager*, also `devmgmt.msc`): devices by type with
  their status, driver and IDs; enable, disable, uninstall, scan for hardware changes, show hidden devices.
  Opens instantly (no MMC console).
- **Pinned apps**: right-click an app in the launcher, a window button, a desktop icon or a program in the
  file manager → *Pin to taskbar*. Windows of a pinned app group under its icon (click to switch, middle
  click for a new window, right-click for a menu).
- **Properties** (`delfin.exe --props <file>`, right-click → *Properties*, Alt+Enter in the file manager):
  General, Shortcut (edit target, arguments, start in…), Compatibility (compatibility mode, run as
  administrator, high DPI…) and Details (version information, 32/64-bit).
- **Settings** (`delfin.exe --settings [page]`, launcher → *Settings*, every `ms-settings:` link): Delfin's
  take on Windows 11 Settings, which can't start without Explorer. System (display, sound, power plans and
  sleep, storage and cleanup, about / rename PC), Bluetooth & devices, mouse, network & Wi-Fi,
  personalization (wallpaper, dark mode), installed apps (uninstall), startup apps, accounts, time zone and
  language, Windows Update (check, install, history) and Delfin's own options. Search with Ctrl+F.
- **What Explorer used to do**: app notifications (pop-ups and a notification centre behind the bell,
  Do not disturb, `Win+N`), window snapping (`Win+arrows`, dragging to an edge or corner with a preview),
  `Win+X` menu, `Win+I` Settings, `Win+A` quick settings, `Win+1…9` pinned apps, the keyboard language
  indicator and safely removing USB drives. Sound devices are switched in Settings and quick settings.
- **Archives** (right-click in the file manager or on the desktop): *Extract Here*, *Extract to "name\"*,
  *Open in 7-Zip* and *Add to "name.zip"* / *"name.7z"*. Uses 7-Zip when it is installed (its progress
  window, password and overwrite questions), otherwise the `tar` that comes with Windows.
- **Shortcuts**: `Win+E` file manager, `Win+D` show desktop, `Win+R` / `Alt+F1` launcher, `Alt+Tab` window
  switcher, `Ctrl+Shift+Esc` task manager.
- Folders opened anywhere in Windows open in Delfin.

### Try it without installing

```
taskkill /f /im explorer.exe
target\release\delfin.exe --shell
```
To get Explorer back: Delfin settings → *Back to Explorer*, or Ctrl+Alt+Del → Task Manager →
*Run new task* → `explorer.exe`.

### Install

Download **`delfin-setup.exe`** from [Releases](https://github.com/Atimaniac/kde/releases) and run it
(or run `delfin.exe --setup`). The installer:

- copies Delfin to `%LOCALAPPDATA%\Programs\Delfin\delfin.exe` (per user, no admin rights),
- makes Delfin the Windows shell **and** the file manager: panel, launcher, desktop and tray at
  sign-in, and folders opened anywhere in Windows open in Delfin (both can be switched off),
- adds a Start menu shortcut (and a desktop one if you want), and an entry in Settings › Apps
  that uninstalls it again (`delfin.exe --uninstall`),
- at the end, switches from Explorer to Delfin right away, or at your next sign-in.

Running the installer again over an existing installation updates it, even while Delfin runs.
Without the installer: Delfin menu (☰) → *System* → **Use Delfin as Windows shell…**, or
`delfin.exe --install-shell`, then sign out and back in.

**Go back:** Delfin settings → *Back to Explorer* (click twice), or `delfin.exe --uninstall-shell`.
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
- **Windows file clipboard**: files copied or cut in Delfin can be pasted in Explorer, on the desktop, in another Delfin window or in any program that takes files, and the other way round
- **Right-click menu** of a file: Open With…, Run as administrator, Edit in Notepad, Set as Wallpaper, Paste Into Folder, Duplicate (Ctrl+D), Copy Location / Copy Name, Create Shortcut (here or on the desktop), archives, Pin to taskbar, Properties. Of the folder itself: Create New (folder, text document), Paste, View Mode, Sort By, Open in New Window, Properties
- **Type to find**: typing letters jumps to the file whose name starts with them; the same letter again goes to the next one
- Folders are read in the background: a slow one (network drive, sleeping disk) shows *Loading…* instead of freezing the window
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
| Ctrl+D | Duplicate |
| Letters | Jump to the file that starts with them |
| Del / Shift+Del | Trash / Delete permanently |

## Project layout

- `src/app.rs`: window, panels, views, actions, shortcuts
- `src/theme.rs`: Breeze colours and fonts
- `src/icons.rs`: vector-painted Breeze-style icons
- `src/fsops.rs`: listing, sorting, copy/move/delete
- `src/shellops.rs`: file clipboard, Open With, Run as administrator, shortcuts, wallpaper
- `src/renamebox.rs`: the small rename window the desktop uses
- `src/platform.rs`: Windows drives, free space, terminal
- `src/config.rs`: settings file
