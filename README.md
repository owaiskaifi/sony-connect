# Sony Connect

A small Linux desktop + tray app (and CLI) for controlling Sony Bluetooth headphones:
battery, volume, noise cancelling / ambient sound, and ULT bass mode, without a phone.

> **Disclaimer: Unofficial; not affiliated with or endorsed by Sony. Use at your own risk.**
> "Sony", "ULT WEAR" and related names are trademarks of Sony Group Corporation and are used
> here only to describe compatibility.

## Supported devices

| Device | Status |
| --- | --- |
| Sony ULT WEAR (WH-ULT900N) | Tested |
| Other Sony headsets using the MDR v2 protocol (e.g. WH-1000XM5/XM6, WF-1000XM5, WH-CH720N). Older v1 devices such as the WH-1000XM3/XM4 are not supported | May work, untested. Reports welcome |

## Features

- **Battery** level (via BlueZ, with the Sony protocol as a fallback)
- **Volume** control
- **Noise cancelling / ambient sound**: off, noise cancelling, ambient with level and voice focus
- **ULT mode**: off, ULT 1, ULT 2
- **Tray icon** with quick toggles
- **CLI** (`sony-connect-cli`) for scripting, with JSON output

## Install

### Arch Linux

AUR package coming later. For now, build with `makepkg`:

```sh
git clone https://github.com/owaiskaifi/sony-connect.git
cd sony-connect/packaging/arch        # tagged release
# or: cd sony-connect/packaging/arch-git  (latest git, package sony-connect-git)
makepkg -si
```

Each [release](https://github.com/owaiskaifi/sony-connect/releases) also includes a prebuilt
`.pkg.tar.zst` (`sudo pacman -U sony-connect-*.pkg.tar.zst`).

### Debian / Ubuntu

Download the `.deb` from [Releases](https://github.com/owaiskaifi/sony-connect/releases) and install it:

```sh
sudo apt install ./Sony.Connect_*_amd64.deb
```

### AppImage (other distros)

```sh
chmod +x Sony.Connect_*_amd64.AppImage
./Sony.Connect_*_amd64.AppImage
```

The standalone CLI binary (`sony-connect-cli-<version>-x86_64-linux`) is also attached to each release.

### Requirements

- BlueZ (`bluetoothd`) running, headset already paired and connected
  (e.g. via `bluetoothctl` or your desktop's Bluetooth settings)
- A tray implementation supporting AppIndicator/StatusNotifierItem
  (GNOME needs the AppIndicator extension)

## Build from source

Dependencies:

- **Arch:** `sudo pacman -S --needed base-devel rust nodejs npm webkit2gtk-4.1 gtk3 libayatana-appindicator bluez bluez-libs dbus`
- **Debian/Ubuntu (22.04+):**
  `sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libbluetooth-dev libdbus-1-dev nodejs npm`
  plus Rust from [rustup](https://rustup.rs)

Build:

```sh
git clone https://github.com/owaiskaifi/sony-connect.git
cd sony-connect

# CLI
cargo build --release -p sony-core --bin sony-connect-cli
# -> target/release/sony-connect-cli

# Desktop app
cd app
npm ci
npm run tauri build -- --no-bundle            # binary only
# npm run tauri build -- --bundles deb,appimage  # with packages
# npm run tauri dev                            # development
```

## CLI usage

```text
sony-connect-cli list                                  # list paired/connected Sony devices
sony-connect-cli status [--json]                       # battery, volume, ANC and ULT state
sony-connect-cli anc <off|nc|ambient> [--level N] [--voice]
sony-connect-cli ult <off|1|2>
sony-connect-cli volume <0-100>
```

Examples:

```sh
sony-connect-cli anc ambient --level 10 --voice   # ambient sound, level 10, voice focus on
sony-connect-cli anc nc                            # noise cancelling
sony-connect-cli ult 2
sony-connect-cli volume 40
sony-connect-cli status --json | jq .battery
```

## Roadmap

- [x] Linux: Arch package, `.deb`, AppImage
- [ ] Windows support (next)
- [ ] AUR package
- [ ] Equalizer and more device settings
- [ ] More tested devices

## Credits

Protocol research by these projects made this possible:

- [mos9527/SonyHeadphonesClient](https://github.com/mos9527/SonyHeadphonesClient)
- [Plutoberth/SonyHeadphonesClient](https://github.com/Plutoberth/SonyHeadphonesClient)

Built with [Tauri](https://tauri.app) and [BlueZ](https://www.bluez.org).

## License

[MIT](LICENSE) © 2026 owais. See [CONTRIBUTING.md](CONTRIBUTING.md) to contribute.
