# sony-connect — shared facts for all contributors

- Product name: **Sony Connect**; binary/package name: `sony-connect`; app identifier: `dev.owais.sonyconnect`
- Layout: `crates/sony-core` (lib + `sony-connect-cli` CLI bin), `app/` (Tauri v2: `app/src-tauri` Rust, `app/ui` static HTML/CSS/JS, no bundler), `packaging/arch/PKGBUILD`, `.github/workflows/`
- Targets for v0.1: Arch (PKGBUILD) + Debian (.deb) + AppImage. Windows later — no Windows code yet.
- Battery + volume on Linux come from BlueZ D-Bus (`org.bluez.Battery1.Percentage`, `org.bluez.MediaTransport1.Volume` 0-127) — proven working. MDR protocol fills ANC/ambient/ULT (and battery/volume as fallback).
- Test device: ULT WEAR (WH-ULT900N), A8:E6:E8:6C:6B:B1, Modalias usb:v054Cp0F1E.
- Not affiliated with Sony; README must say so.
- No commits by contributors; the maintainer commits. No AI co-author trailers.
