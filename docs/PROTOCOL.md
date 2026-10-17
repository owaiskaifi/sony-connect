# Sony MDR protocol notes (v2, as used by ULT WEAR / WH-ULT900N)

Reference implementation: [mos9527/SonyHeadphonesClient](https://github.com/mos9527/SonyHeadphonesClient)
(`master`, C++, `libmdr/` + `libmdr-bt/`). It was used as reference data only. Paths below are relative
to that repo. Byte values in **bold** were confirmed on a real ULT WEAR (see the last section).

## 1. Transport and channel discovery

| | |
|---|---|
| Bluetooth | Classic BR/EDR, RFCOMM (SPP-like stream) |
| MDR v2 service UUID | `956C7B26-D49A-4BA8-B03F-B17D393CB6E2` (WH-1000XM5 and newer, ULT WEAR) — `libmdr/include/mdr-c/Base.h` `MDR_SERVICE_UUID_V2` |
| MDR v1 service UUID | `96CC203E-5068-46AD-B32D-E316F5E069BA` (XM4 and older) — `MDR_SERVICE_UUID_V1`. **Not advertised by the ULT WEAR**; not implemented here. |
| BLE GATT service | `5B833E20-6BC7-4802-8E9A-723CECA4BD8F` (`MDR_SERVICE_UUID_BLE`); the ULT WEAR lists `5b833e06-…`/`5b833e26-…` siblings. Not used. |

How the reference's Linux code finds the channel (`libmdr-bt/src/Linux/ConnectionLinux.cpp`,
`DBusHelper.cpp`): **SDP**, not `RegisterProfile` and not hard-coded. It opens a non-blocking
SDP session (`sdp_connect`), runs `sdp_service_search_attr_req` for the 128-bit service UUID,
takes the port from the record's ProtocolDescriptorList with `sdp_get_proto_port(.., RFCOMM_UUID)`,
then `connect()`s an `AF_BLUETOOTH/SOCK_STREAM/BTPROTO_RFCOMM` socket to that channel with
`RFCOMM_LM_AUTH | RFCOMM_LM_ENCRYPT`. Device listing comes from BlueZ D-Bus.

This crate does the same without libbluetooth: a ~100-line SDP client over an L2CAP
`SOCK_SEQPACKET` socket to PSM 1 (`crates/sony-core/src/transport/sdp.rs`), a
ServiceSearchAttributeRequest (PDU 0x06) for the v2 UUID with attribute range 0x0000-0xFFFF,
then the RFCOMM uint8 from attribute 0x0004. **On this ULT WEAR the channel is 18**
(record name "SPP Serial Port"). Don't hard-code it — it is firmware-specific; the env var
`SONY_CONNECT_RFCOMM_CHANNEL` exists only for debugging. `bdaddr_t` is little-endian
(`A8:E6:E8:6C:6B:B1` → `B1 6B 6C E8 E6 A8`).

## 2. Framing

`libmdr/include/mdr/Command.hpp`, `libmdr/src/Command.cpp` (`MDRPackCommand` / `MDRUnpackCommand`).

```
3E | escape( dataType | seq | len (u32 BE) | payload[len] | checksum ) | 3C
```

* `checksum` = 8-bit wrapping sum of `dataType, seq, the 4 length bytes, payload` (before escaping).
* Escaping (applies to everything between the markers, including the checksum):
  `3C → 3D 2C`, `3D → 3D 2D`, `3E → 3D 2E` (i.e. `3D, b-0x10`). So `3E`/`3C` never occur inside a frame.
* Data types (`MDRDataType`, `libmdr/include/mdr/Protocol.hpp`): `00 DATA`, `01 ACK`, `02 DATA_MC_NO1`,
  `09 DATA_ICD`, `0A DATA_EV`, **`0C DATA_MDR` (= "table 1")**, `0D DATA_COMMON`,
  **`0E DATA_MDR_NO2` (= "table 2")**, `10..1E` SHOT variants, `2D LARGE_DATA_COMMON`.
* Sequence numbers are 1 bit (0/1) (`libmdr/src/Headphones.cpp`):
  * The host sends a command with its current `seq`. The device answers with an **ACK whose seq is `1 - seq`**;
    the host then adopts that value as its next seq (`HandleAck`).
  * Every non-ACK frame received must be ACKed with `seq = 1 - rx_seq` and an empty payload (`SendACK`).
  * No ACK → resend the same frame with the same seq (reference: 10 tries × 1000 ms, `Details.hpp`
    `SendCommandACK`; this crate: 3 × 1000 ms).
* Example: GET protocol info, seq 0 → `3E 0C 00 00 00 00 02 00 00 0E 3C`; ACK with seq 1 → `3E 01 01 00 00 00 00 02 3C`.
* Max packet 2048 bytes (`kMDRMaxPacketSize`).

Payload encoding (`Protocol.hpp`): byte 0 is the command id, byte 1 usually an "inquired type"
selecting the struct variant. Strings are `u8 length + bytes` (`MDRPrefixedString`), arrays are
`u8 count + items` (`MDRPodArray`). In v2, `EnableDisable`/`OnOffSettingValue` use **`00 = ENABLE/ON`,
`01 = DISABLE/OFF`** (inverted!), while `NcAsmOnOffValue` uses `00 = OFF, 01 = ON`.

## 3. Init handshake, T1 vs T2

`libmdr/src/HeadphonesV2.cpp` `RequestInitV2`. Protocol family (v1/v2) is chosen by which service
UUID was connected; the v2 init is:

1. `0C` **`00 00`** CONNECT_GET_PROTOCOL_INFO → **`01 00 <version u32 BE> <table1 EnableDisable> <table2 EnableDisable>`**.
   ULT WEAR: **`01 00 03 00 20 02 00 00`** → version `0x03002002`, table 1 and table 2 enabled.
2. `0C 02 00` CONNECT_GET_CAPABILITY_INFO (unique id; optional).
3. `0C 04 <type>` CONNECT_GET_DEVICE_INFO, type `01` model name, `02` FW version, `03` series+color.
4. `0C 06 00` CONNECT_GET_SUPPORT_FUNCTION (table 1) and `0E 06 00` (table 2) →
   `07 00 <n> (<functionType> <priority>)×n`. Everything else is gated on these function ids
   (`libmdr/include/mdr/ProtocolV2.hpp` `t1::FunctionType` / `t2::FunctionType`).
5. Then per-feature GETs, and `C4` LOG_SET_STATUS to enable operation-log notifications (optional).

T1 (`DATA_MDR`, `ProtocolV2T1.hpp`) carries all the features we need (battery, NC/ASM, EQ/ULT,
playback volume, device info). T2 (`DATA_MDR_NO2`, `ProtocolV2T2.hpp`) carries peripheral/pairing
management, voice guidance, safe listening, etc. **The ULT WEAR needs only T1** for this app.

ULT WEAR table-1 support functions (fw 2.1.0):
`10 12 13 14 15 20 23 25 27 32 53 69 6B 70 93 F4 90 A1 F1 B2 B7 C1 C2 D1 D2 D3 E3 E2 F9 FD` — notably
`20` BATTERY_LEVEL_INDICATOR, `53` PRESET_EQ_AND_ULT_MODE, `69` AMBIENT_SOUND_CONTROL_MODE_SELECT,
`6B` MODE_NC_ASM_NOISE_CANCELLING_DUAL_AMBIENT_SOUND_MODE_LEVEL_ADJUSTMENT, `70` AUTO_NCASM,
`A1` PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT. Table 2: `42 50 F2`.

## 4. Commands (table 1, data type `0C`)

Command ids: `ProtocolV2T1.hpp` `enum class Command`. Pattern: GET = x, RET = x+1, SET = x+2, NTFY = x+3
(e.g. NCASM `66/67/68/69`). Replies are matched by `command+1` and the same inquired type.

### Model / firmware (`ConnectRetDeviceInfo*`)
* `04 01` → **`05 01 08 "ULT WEAR"`**; `04 02` → **`05 02 05 "2.1.0"`**.

### Battery (`PowerGetStatus`, `PowerRetStatus*`; handler `HandlePowerStatusT1`)
`PowerInquiredType`: `00` BATTERY, `01` LEFT_RIGHT_BATTERY, `02` CRADLE_BATTERY, `08` BATTERY_WITH_THRESHOLD,
`09` LR_BATTERY_WITH_THRESHOLD, `0A` CRADLE_BATTERY_WITH_THRESHOLD. Pick by support function
(`20`/`28` single, `21`/`29` L+R, `22`/`2A` case).
* Single: `22 00` → **`23 00 <level 0-100> <charging>`** (`BatteryChargingStatus`: `00` not, `01` charging, `02` unknown, `03` charged). NTFY: `25 00 …`.
* With threshold: `23 08 <level> <charging> <threshold>`.
* Dual (TWS): `23 01 <L level> <L charging> <R level> <R charging>`.

### NC / ambient sound (`NcAsm*`; handler `HandleNcAsmParamT1`)
`NcAsmInquiredType` relevant values: `17` MODE_NC_ASM_DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS (support `6B`),
`19` …_NA (support `6D`, adds noise-adaptation bytes), `22` ASM_SEAMLESS (support `67`, ambient only), `30` NC_AMB_TOGGLE (button function, support `69`).
* GET `66 17` → RET `67 17 <valueChange> <total> <mode> <asmMode> <level>`; NTFY `69 17 …` (same layout).
* SET `68 17 <valueChange> <total> <mode> <asmMode> <level>`:
  * `valueChange`: `00` UNDER_CHANGING (slider dragging), `01` CHANGED.
  * `total` (ncAsmTotalEffect): `00` off, `01` on.
  * `mode` (NcAsmMode): `00` NC, `01` ASM (ambient).
  * `asmMode` (AmbientSoundMode): `00` normal, `01` voice (focus on voice).
  * `level`: ambient level `00..14` (0-20).
  * ANC on: **`68 17 01 01 00 00 14`**; ANC off: **`68 17 01 00 00 00 14`**; ambient level 10: **`68 17 01 01 01 00 0A`**; ambient 12 + voice: `68 17 01 01 01 01 0C`.
* `_NA` variant (`19`) appends `<autoAsm on/off> <NoiseAdaptiveSensitivity>`; ASM-only (`22`) is `68 22 <vc> <total> <asmMode> <level>`.

### ULT mode — part of the EQ command, not its own command
Grep for "ULT" in `Generated/ProtocolV2T1Enum.hpp` / `ProtocolV2T1.hpp`: `EqEbbInquiredType::PRESET_EQ_AND_ULT_MODE = 0x03`,
`EqUltModeStatus { OFF=00, ULT_1=01, ULT_2=02 }`, struct `EqParamUltMode { presetId, eqUltModeStatus, bandSteps[] }`,
`EqEbbRetParamEqAndUltMode` / `EqEbbSetParamEqAndUltMode` / `EqEbbNtfyParamEqAndUltMode`, support function `53`.
(Also `SoundEffectType::SOUND_EFFECT_ULT/ULT1/ULT2` for the newer "sound effect" EQ (`EqEbbInquiredType 30`, support `54`),
`ULT_BTN_SOUND_EFFECT_ASSIGN 40`, and T2 `SOUNDEFFECT_ULT_BEEP_ONOFF` — none of those are advertised by the ULT WEAR.)
The reference parses the EQ reply as plain `EqParam` and never sets ULT, so it would misparse this device; the layout below is from the structs + live captures.

* GET `56 03` (or `56 00`) → **`57 03 <preset> <ult> <n> <band×n>`**; ULT WEAR: **`57 03 00 02 06 0A 0A 0A 0A 0A 0A`** (preset OFF, ULT 2, 6 flat bands).
  The device answers `56 00` with the same ULT layout (`57 00 00 02 06 …`).
* SET `58 03 <preset> <ult> <n> <band×n>` → ACK, then NTFY **`59 03 <preset> <ult> <n> <bands>`**.
  Do a read-modify-write: echo the preset + bands just read and change only `<ult>`.
  E.g. ULT 1: **`58 03 00 01 06 0A 0A 0A 0A 0A 0A`**.
* EQ capability: `50 03` → `51 03 <bands=06> <steps=15> <ult2AdditionalStep=0A> <npresets=0C> (<id> <name str>)…`
  (`EqEbbRetCapabilityEqAndUltMode`); band value `0A` is the 0 dB centre.
* `EqPresetId`: `00` OFF, `10` BRIGHT … `17` SPEECH, `A0` CUSTOM, `A1..A5` USER_SETTING1-5.

### Volume (`GetPlayParam`, `SetPlayParamPlaybackControllerVolume`; handler `HandlePlayParamT1`)
`PlayInquiredType::MUSIC_VOLUME = 0x20` (support `A1`..`A4`).
* GET `A6 20` → **`A7 20 <volume>`**; SET **`A8 20 <volume>`** → NTFY **`A9 20 <volume>`**.
* Scale on the ULT WEAR: **0-30** (12 ≙ BlueZ AVRCP 51/127 ≙ 40 %). The device keeps it in sync with the A2DP absolute volume.

## 5. Linux system integration (BlueZ D-Bus)
* Discovery: `org.freedesktop.DBus.ObjectManager.GetManagedObjects` on `org.bluez /`, `org.bluez.Device1` with `Paired`
  and Modalias containing `v054C` (Sony), else the MDR UUIDs, else name prefixes (WH-/WF-/WI-/ULT WEAR/LinkBuds/INZONE).
* Battery: `org.bluez.Battery1.Percentage` on the device path (HFP-reported, coarse; MDR gives 1 % steps). No charging flag → taken from MDR.
* Volume: `org.bluez.MediaTransport1.Volume` (0-127) on the transport whose `Device` is our device path (`…/sepN/fdM`, path varies).
* Public API: `DeviceState.volume` is percent 0-100 (BlueZ first, else MDR), `set_volume` writes the BlueZ transport volume
  when an A2DP transport exists, else MDR `A8 20`.

## Verified on ULT WEAR

WH-ULT900N "ULT WEAR", fw **2.1.0**, Modalias `usb:v054Cp0F1Ed0210`, Linux/BlueZ, 2026-10-17. Tested with
`sony-connect-cli` and a Python probe; all settings were restored to the starting state
(NC on, ambient level 20, voice off, ULT 2, EQ preset OFF/flat, volume 51/127).

| Feature | Result |
|---|---|
| SDP lookup of `956C7B26-…` | works, RFCOMM channel **18** |
| RFCOMM connect + protocol info / support functions / model / FW | works (whole `status` ≈ 1.5 s incl. SDP) |
| Battery (MDR `22 00`) | works (`23 00 43 00` = 67 %, while BlueZ said 70 %) |
| NC/ASM get / set / notify (`66/68/69 17`) | works: NC, off, ambient level 10 and 12, focus-on-voice |
| ULT get / set / notify (`56/58/59 03`) | works: off, ULT 1, ULT 2 via read-modify-write |
| Volume get/set via MDR `A6/A8 20` | works; BlueZ transport volume follows (12/30 ↔ 51/127) |
| Volume set via BlueZ `MediaTransport1.Volume` | works; MDR volume follows (38/127 ↔ 9/30) |
| BlueZ `Battery1.Percentage` | works (70) |

Gotchas found live:
* **In NC mode the ambient fields are ignored.** `68 17 01 01 00 00 14` while the stored ambient state was level 12 + voice
  produced NTFY `69 17 01 01 00 01 0C` — the device keeps the old level/voice. Set them while in ambient mode.
* **A malformed EQ SET corrupts the custom EQ.** Sending `58 00 00 00 06` (band count 6 but no band bytes) was accepted:
  the device switched to preset `A0` CUSTOM with garbage bands `70 3C 00 00 94 44` (it read past the payload) and ULT went
  to off. Recovered with `58 00 A0 02 06 0A×6` (flat custom bands) then `58 00 00 02 00` (preset OFF, ULT 2, no bands).
  A SET with no array byte at all (`58 00 00 02`) is silently dropped (no ACK). With inquired type `03` the device
  ignores preset *and* bands and only applies the ULT byte (`58 03 00 01 06 0A×6` while holding `A0 … 70 3C 00 00 94 44`
  returned `59 03 A0 01 06 70 3C 00 00 94 44`): type `03` is effectively a ULT-only setter, type `00` the full EQ
  setter. `set_ult` uses type `03` with the echoed bytes, so it cannot clobber a custom EQ.
  Side effect of that incident: the user's stored CUSTOM EQ bands (unknown before) are now flat.
* Only one RFCOMM client at a time: a second `connect` while another process holds channel 18 fails with
  `EBUSY` ("Device or resource busy", os error 16). The app and the CLI cannot be connected simultaneously.
* After `58 03 …` the NTFY comes back as `59 03`, even when the GET used type `00`.
