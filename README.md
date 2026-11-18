# Delfin — a KDE Dolphin-style file manager for Windows (Rust)

Breeze Dark / Breeze Light look, written in Rust with egui. It doesn't replace Windows Explorer; it runs as a normal app next to it.

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
