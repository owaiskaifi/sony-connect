//! Sony MDR (v2) protocol: pure framing + command builders/parsers. No I/O.
//!
//! See docs/PROTOCOL.md for the byte-level description and sources.
//!
//! Frame on the wire:
//! ```text
//! 0x3E | escape( type | seq | len:u32be | payload[len] | checksum ) | 0x3C
//! ```
//! `checksum` = wrapping sum of `type, seq, len bytes, payload`. Bytes 0x3C/0x3D/0x3E
//! inside the escaped region become `0x3D, b - 0x10`.

pub const START: u8 = 0x3E;
pub const END: u8 = 0x3C;
pub const ESCAPE: u8 = 0x3D;

/// RFCOMM SDP service UUID for MDR v2 (WH-1000XM5 and newer, ULT WEAR).
pub const SERVICE_UUID_V2: &str = "956c7b26-d49a-4ba8-b03f-b17d393cb6e2";
/// RFCOMM SDP service UUID for MDR v1 (WH-1000XM4 and older). Not implemented.
pub const SERVICE_UUID_V1: &str = "96cc203e-5068-46ad-b32d-e316f5e069ba";

/// Frame data types (`MDRDataType` in the reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataType {
    Data = 0x00,
    Ack = 0x01,
    /// MDR "table 1" commands.
    DataMdr = 0x0C,
    DataCommon = 0x0D,
    /// MDR "table 2" commands.
    DataMdrNo2 = 0x0E,
}

impl DataType {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x00 => Self::Data,
            0x01 => Self::Ack,
            0x0C => Self::DataMdr,
            0x0D => Self::DataCommon,
            0x0E => Self::DataMdrNo2,
            _ => return None,
        })
    }
}

/// A decoded frame. `data_type` is kept raw so unknown types survive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub data_type: u8,
    pub seq: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn is_ack(&self) -> bool {
        self.data_type == DataType::Ack as u8
    }
}

fn escape_into(out: &mut Vec<u8>, b: u8) {
    if matches!(b, START | END | ESCAPE) {
        out.push(ESCAPE);
        out.push(b - 0x10);
    } else {
        out.push(b);
    }
}

/// Encode a frame (start/end markers, escaping, checksum).
pub fn encode(data_type: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(payload.len() + 7);
    raw.push(data_type);
    raw.push(seq);
    raw.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    raw.extend_from_slice(payload);
    let sum = raw.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    raw.push(sum);
    let mut out = Vec::with_capacity(raw.len() * 2 + 2);
    out.push(START);
    for b in raw {
        escape_into(&mut out, b);
    }
    out.push(END);
    out
}

/// ACK for a received frame with sequence number `rx_seq`.
pub fn encode_ack(rx_seq: u8) -> Vec<u8> {
    encode(DataType::Ack as u8, 1 - (rx_seq & 1), &[])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    BadEscape,
    TooShort,
    BadLength,
    BadChecksum,
}

/// Decode the bytes *between* START and END markers.
pub fn decode_body(body: &[u8]) -> Result<Frame, DecodeError> {
    let mut raw = Vec::with_capacity(body.len());
    let mut it = body.iter();
    while let Some(&b) = it.next() {
        if b == ESCAPE {
            match it.next() {
                Some(&e) if (0x2C..=0x2E).contains(&e) => raw.push(e + 0x10),
                _ => return Err(DecodeError::BadEscape),
            }
        } else {
            raw.push(b);
        }
    }
    if raw.len() < 7 {
        return Err(DecodeError::TooShort);
    }
    let len = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]) as usize;
    if raw.len() != len + 7 {
        return Err(DecodeError::BadLength);
    }
    let (data, sum) = raw.split_at(raw.len() - 1);
    let calc = data.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    if calc != sum[0] {
        return Err(DecodeError::BadChecksum);
    }
    Ok(Frame {
        data_type: raw[0],
        seq: raw[1],
        payload: raw[6..6 + len].to_vec(),
    })
}

/// Incremental stream decoder: push received bytes, pop complete frames.
/// Malformed frames and garbage between frames are dropped.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Next complete, valid frame, if any.
    pub fn next_frame(&mut self) -> Option<Frame> {
        loop {
            let start = match self.buf.iter().position(|&b| b == START) {
                Some(s) => s,
                None => {
                    self.buf.clear();
                    return None;
                }
            };
            if start > 0 {
                self.buf.drain(..start);
            }
            let end = self.buf.iter().skip(1).position(|&b| b == END)? + 1;
            let body: Vec<u8> = self.buf[1..end].to_vec();
            self.buf.drain(..=end);
            // A START inside the body means we lost an END: resync from it.
            if let Some(p) = body.iter().rposition(|&b| b == START) {
                if let Ok(f) = decode_body(&body[p + 1..]) {
                    return Some(f);
                }
                continue;
            }
            if let Ok(f) = decode_body(&body) {
                return Some(f);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Table 1 command ids (ProtocolV2T1.hpp `Command`)
// ---------------------------------------------------------------------------

pub mod cmd {
    pub const CONNECT_GET_PROTOCOL_INFO: u8 = 0x00;
    pub const CONNECT_RET_PROTOCOL_INFO: u8 = 0x01;
    pub const CONNECT_GET_DEVICE_INFO: u8 = 0x04;
    pub const CONNECT_RET_DEVICE_INFO: u8 = 0x05;
    pub const CONNECT_GET_SUPPORT_FUNCTION: u8 = 0x06;
    pub const CONNECT_RET_SUPPORT_FUNCTION: u8 = 0x07;
    pub const POWER_GET_STATUS: u8 = 0x22;
    pub const POWER_RET_STATUS: u8 = 0x23;
    pub const POWER_NTFY_STATUS: u8 = 0x25;
    pub const EQEBB_GET_PARAM: u8 = 0x56;
    pub const EQEBB_RET_PARAM: u8 = 0x57;
    pub const EQEBB_SET_PARAM: u8 = 0x58;
    pub const EQEBB_NTFY_PARAM: u8 = 0x59;
    pub const NCASM_GET_PARAM: u8 = 0x66;
    pub const NCASM_RET_PARAM: u8 = 0x67;
    pub const NCASM_SET_PARAM: u8 = 0x68;
    pub const NCASM_NTFY_PARAM: u8 = 0x69;
    pub const PLAY_GET_PARAM: u8 = 0xA6;
    pub const PLAY_RET_PARAM: u8 = 0xA7;
    pub const PLAY_SET_PARAM: u8 = 0xA8;
    pub const PLAY_NTFY_PARAM: u8 = 0xA9;
}

/// Table 1 `FunctionType` values we care about (ProtocolV2.hpp `t1::FunctionType`).
pub mod func {
    pub const BATTERY_LEVEL_INDICATOR: u8 = 0x20;
    pub const LEFT_RIGHT_BATTERY_LEVEL_INDICATOR: u8 = 0x21;
    pub const BATTERY_LEVEL_WITH_THRESHOLD: u8 = 0x28;
    pub const PRESET_EQ: u8 = 0x50;
    pub const PRESET_EQ_AND_ULT_MODE: u8 = 0x53;
    pub const AMBIENT_SOUND_MODE_LEVEL_ADJUSTMENT: u8 = 0x67;
    pub const AMBIENT_SOUND_CONTROL_MODE_SELECT: u8 = 0x69;
    pub const NC_ASM_DUAL_LEVEL: u8 = 0x6B;
    pub const NC_ASM_DUAL_LEVEL_NOISE_ADAPTATION: u8 = 0x6D;
    pub const PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT: u8 = 0xA1;
    pub const PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT_AND_MUTE: u8 = 0xA2;
    pub const PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT_AND_FUNCTION_CHANGE: u8 = 0xA3;
    pub const PLAYBACK_CONTROLLER_WITH_FUNCTION_CHANGE: u8 = 0xA4;
}

/// `PowerInquiredType`.
pub mod power {
    pub const BATTERY: u8 = 0x00;
    pub const LEFT_RIGHT_BATTERY: u8 = 0x01;
    pub const BATTERY_WITH_THRESHOLD: u8 = 0x08;
}

/// `NcAsmInquiredType`.
pub mod ncasm {
    /// NC on/off + NC/ASM mode + seamless ambient level (WH-1000XM5, ULT WEAR).
    pub const DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS: u8 = 0x17;
    /// Same plus noise-adaptation fields.
    pub const DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS_NA: u8 = 0x19;
    /// Ambient only, no NC.
    pub const ASM_SEAMLESS: u8 = 0x22;
}

/// `EqEbbInquiredType`.
pub mod eq {
    pub const PRESET_EQ: u8 = 0x00;
    pub const PRESET_EQ_AND_ULT_MODE: u8 = 0x03;
}

/// `PlayInquiredType::MUSIC_VOLUME`.
pub const PLAY_MUSIC_VOLUME: u8 = 0x20;

/// `DeviceInfoType`.
pub const DEVICE_INFO_MODEL_NAME: u8 = 0x01;
pub const DEVICE_INFO_FW_VERSION: u8 = 0x02;

// ---------------------------------------------------------------------------
// Builders (payloads only; wrap with `encode(DataType::DataMdr, seq, ..)`)
// ---------------------------------------------------------------------------

pub fn get_protocol_info() -> Vec<u8> {
    vec![cmd::CONNECT_GET_PROTOCOL_INFO, 0x00]
}

pub fn get_device_info(info_type: u8) -> Vec<u8> {
    vec![cmd::CONNECT_GET_DEVICE_INFO, info_type]
}

pub fn get_support_function() -> Vec<u8> {
    vec![cmd::CONNECT_GET_SUPPORT_FUNCTION, 0x00]
}

pub fn get_battery(power_type: u8) -> Vec<u8> {
    vec![cmd::POWER_GET_STATUS, power_type]
}

pub fn get_ncasm(inquired: u8) -> Vec<u8> {
    vec![cmd::NCASM_GET_PARAM, inquired]
}

/// NC/ASM settings in device terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NcAsm {
    /// `ncAsmTotalEffect` == ON.
    pub enabled: bool,
    /// `NcAsmMode::ASM` (ambient) when true, `NC` when false.
    pub ambient: bool,
    /// `AmbientSoundMode::VOICE`.
    pub focus_on_voice: bool,
    /// 0..=20.
    pub level: u8,
}

/// `NcAsmSetParamModeNcDualModeSwitchAsmSeamless` (inquired 0x17):
/// `68 17 <valueChange=01> <total on=01/off=00> <mode NC=00/ASM=01> <voice 00/01> <level>`.
pub fn set_ncasm_dual(s: NcAsm) -> Vec<u8> {
    vec![
        cmd::NCASM_SET_PARAM,
        ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS,
        0x01, // ValueChangeStatus::CHANGED
        s.enabled as u8,
        s.ambient as u8,
        s.focus_on_voice as u8,
        s.level,
    ]
}

/// `NcAsmSetParamAsmSeamless` (inquired 0x22): ambient-only devices.
pub fn set_ncasm_asm_seamless(s: NcAsm) -> Vec<u8> {
    vec![
        cmd::NCASM_SET_PARAM,
        ncasm::ASM_SEAMLESS,
        0x01,
        s.enabled as u8,
        s.focus_on_voice as u8,
        s.level,
    ]
}

pub fn get_eq(inquired: u8) -> Vec<u8> {
    vec![cmd::EQEBB_GET_PARAM, inquired]
}

/// Preset-EQ + ULT parameter (`EqParamUltMode`): `preset, ult, n, bands[n]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqUlt {
    pub preset: u8,
    /// `EqUltModeStatus`: 0 = off, 1 = ULT 1, 2 = ULT 2.
    pub ult: u8,
    pub bands: Vec<u8>,
}

/// `EqEbbSetParamEqAndUltMode`: `58 03 <preset> <ult> <n> <bands..>`.
///
/// Always echo back the preset and bands just read from the device — a band count
/// that doesn't match the bytes that follow makes the device store garbage as a
/// CUSTOM EQ (see "Verified on ULT WEAR" in docs/PROTOCOL.md).
pub fn set_eq_ult(p: &EqUlt) -> Vec<u8> {
    let mut v = vec![
        cmd::EQEBB_SET_PARAM,
        eq::PRESET_EQ_AND_ULT_MODE,
        p.preset,
        p.ult,
        p.bands.len() as u8,
    ];
    v.extend_from_slice(&p.bands);
    v
}

pub fn get_volume() -> Vec<u8> {
    vec![cmd::PLAY_GET_PARAM, PLAY_MUSIC_VOLUME]
}

/// `SetPlayParamPlaybackControllerVolume`: `a8 20 <0..=30>`.
pub fn set_volume(v: u8) -> Vec<u8> {
    vec![cmd::PLAY_SET_PARAM, PLAY_MUSIC_VOLUME, v]
}

/// Device-side volume range (MDR `MUSIC_VOLUME`), verified on ULT WEAR.
pub const MDR_VOLUME_MAX: u8 = 30;

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// A parsed table-1 message. `Other` keeps anything we don't model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    ProtocolInfo {
        version: u32,
        table1: bool,
        table2: bool,
    },
    ModelName(String),
    FwVersion(String),
    /// Table-1 support function ids (priority bytes dropped).
    SupportFunctions(Vec<u8>),
    /// Battery level 0-100; `charging` from `BatteryChargingStatus` (01 = charging).
    Battery {
        level: u8,
        charging: bool,
    },
    NcAsm(NcAsm),
    EqUlt(EqUlt),
    /// MDR music volume (0..=30).
    Volume(u8),
    Other(Vec<u8>),
}

fn prefixed_string(b: &[u8]) -> Option<String> {
    let n = *b.first()? as usize;
    let s = b.get(1..1 + n)?;
    Some(String::from_utf8_lossy(s).into_owned())
}

/// Parse a table-1 (`DataType::DataMdr`) payload.
pub fn parse(p: &[u8]) -> Message {
    parse_inner(p).unwrap_or_else(|| Message::Other(p.to_vec()))
}

fn parse_inner(p: &[u8]) -> Option<Message> {
    let c = *p.first()?;
    let t = *p.get(1)?;
    Some(match c {
        cmd::CONNECT_RET_PROTOCOL_INFO => {
            let v = p.get(2..8)?;
            Message::ProtocolInfo {
                version: u32::from_be_bytes([v[0], v[1], v[2], v[3]]),
                // EnableDisable: ENABLE = 0x00
                table1: v[4] == 0x00,
                table2: v[5] == 0x00,
            }
        }
        cmd::CONNECT_RET_DEVICE_INFO => match t {
            DEVICE_INFO_MODEL_NAME => Message::ModelName(prefixed_string(&p[2..])?),
            DEVICE_INFO_FW_VERSION => Message::FwVersion(prefixed_string(&p[2..])?),
            _ => return None,
        },
        cmd::CONNECT_RET_SUPPORT_FUNCTION => {
            let n = *p.get(2)? as usize;
            let list = p.get(3..3 + n * 2)?;
            Message::SupportFunctions(list.chunks(2).map(|c| c[0]).collect())
        }
        cmd::POWER_RET_STATUS | cmd::POWER_NTFY_STATUS => match t {
            power::BATTERY | power::BATTERY_WITH_THRESHOLD => Message::Battery {
                level: *p.get(2)?,
                charging: *p.get(3)? == 0x01,
            },
            power::LEFT_RIGHT_BATTERY => {
                // left level, left charging, right level, right charging
                let (l, lc, r, rc) = (*p.get(2)?, *p.get(3)?, *p.get(4)?, *p.get(5)?);
                Message::Battery {
                    level: l.min(r),
                    charging: lc == 1 || rc == 1,
                }
            }
            _ => return None,
        },
        cmd::NCASM_RET_PARAM | cmd::NCASM_NTFY_PARAM => match t {
            ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS
            | ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS_NA => {
                let v = p.get(2..7)?;
                Message::NcAsm(NcAsm {
                    enabled: v[1] == 0x01,
                    ambient: v[2] == 0x01,
                    focus_on_voice: v[3] == 0x01,
                    level: v[4],
                })
            }
            ncasm::ASM_SEAMLESS => {
                let v = p.get(2..6)?;
                Message::NcAsm(NcAsm {
                    enabled: v[1] == 0x01,
                    ambient: true,
                    focus_on_voice: v[2] == 0x01,
                    level: v[3],
                })
            }
            _ => return None,
        },
        // Devices advertising PRESET_EQ_AND_ULT_MODE (0x53) answer both inquired
        // types 0x00 and 0x03 with the EqParamUltMode layout; NTFY uses 0x03.
        cmd::EQEBB_RET_PARAM | cmd::EQEBB_NTFY_PARAM
            if t == eq::PRESET_EQ || t == eq::PRESET_EQ_AND_ULT_MODE =>
        {
            let preset = *p.get(2)?;
            let ult = *p.get(3)?;
            let n = *p.get(4)? as usize;
            let bands = p.get(5..5 + n)?.to_vec();
            Message::EqUlt(EqUlt { preset, ult, bands })
        }
        cmd::PLAY_RET_PARAM | cmd::PLAY_NTFY_PARAM if t == PLAY_MUSIC_VOLUME => {
            Message::Volume(*p.get(2)?)
        }
        _ => return None,
    })
}

/// For a GET payload, does `resp` answer it? (command + 1, same inquired type).
/// EQ GET with type 0x00 may be answered with either 0x00 or 0x03.
pub fn is_response_to(req: &[u8], resp: &[u8]) -> bool {
    if req.len() < 2 || resp.len() < 2 {
        return false;
    }
    if resp[0] != req[0].wrapping_add(1) {
        return false;
    }
    if req[0] == cmd::EQEBB_GET_PARAM {
        return matches!(resp[1], eq::PRESET_EQ | eq::PRESET_EQ_AND_ULT_MODE);
    }
    resp[1] == req[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn encode_protocol_info_request() {
        assert_eq!(
            encode(DataType::DataMdr as u8, 0, &get_protocol_info()),
            hex("3e 0c 00 00 00 00 02 00 00 0e 3c")
        );
    }

    #[test]
    fn encode_ack() {
        assert_eq!(super::encode_ack(0), hex("3e 01 01 00 00 00 00 02 3c"));
        assert_eq!(super::encode_ack(1), hex("3e 01 00 00 00 00 00 01 3c"));
    }

    #[test]
    fn escaping_roundtrip_including_checksum() {
        // payload containing all three special bytes
        let payload = [0x3c, 0x3d, 0x3e, 0x00];
        let f = encode(0x0c, 1, &payload);
        assert_eq!(f[0], START);
        assert_eq!(*f.last().unwrap(), END);
        assert!(!f[1..f.len() - 1].iter().any(|&b| b == START || b == END));
        let d = decode_body(&f[1..f.len() - 1]).unwrap();
        assert_eq!(d.payload, payload);

        // Find a payload whose checksum itself is 0x3c..=0x3e.
        for x in 0u8..=255 {
            let p = [x];
            let raw_sum = 0x0cu8.wrapping_add(0).wrapping_add(1).wrapping_add(x);
            if (0x3c..=0x3e).contains(&raw_sum) {
                let f = encode(0x0c, 0, &p);
                assert_eq!(f[f.len() - 3], ESCAPE);
                assert_eq!(decode_body(&f[1..f.len() - 1]).unwrap().payload, p);
                return;
            }
        }
        panic!("no checksum collision found");
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut f = encode(0x0c, 0, &[0x00, 0x00]);
        let n = f.len();
        f[n - 2] ^= 0xff;
        assert_eq!(decode_body(&f[1..n - 1]), Err(DecodeError::BadChecksum));
    }

    #[test]
    fn decoder_splits_stream() {
        let mut d = Decoder::new();
        let mut stream = vec![0xaa, 0xbb]; // garbage
        stream.extend(encode(0x01, 1, &[]));
        stream.extend(encode(0x0c, 0, &hex("01 00 03 00 20 02 00 00")));
        let (a, b) = stream.split_at(7);
        d.push(a);
        assert!(d.next_frame().is_none());
        d.push(b);
        let f1 = d.next_frame().unwrap();
        assert!(f1.is_ack());
        assert_eq!(f1.seq, 1);
        let f2 = d.next_frame().unwrap();
        assert_eq!(f2.data_type, 0x0c);
        assert!(d.next_frame().is_none());
    }

    // Captured from a ULT WEAR (WH-ULT900N), fw 2.1.0.
    #[test]
    fn parse_captured_responses() {
        assert_eq!(
            parse(&hex("01 00 03 00 20 02 00 00")),
            Message::ProtocolInfo {
                version: 0x0300_2002,
                table1: true,
                table2: true
            }
        );
        assert_eq!(
            parse(&hex("05 01 08 55 4c 54 20 57 45 41 52")),
            Message::ModelName("ULT WEAR".into())
        );
        assert_eq!(
            parse(&hex("05 02 05 32 2e 31 2e 30")),
            Message::FwVersion("2.1.0".into())
        );
        assert_eq!(
            parse(&hex("23 00 43 00")),
            Message::Battery {
                level: 67,
                charging: false
            }
        );
        assert_eq!(
            parse(&hex("67 17 01 01 00 00 14")),
            Message::NcAsm(NcAsm {
                enabled: true,
                ambient: false,
                focus_on_voice: false,
                level: 20
            })
        );
        assert_eq!(parse(&hex("a7 20 0c")), Message::Volume(12));
        let flat = vec![0x0a; 6];
        assert_eq!(
            parse(&hex("57 00 00 02 06 0a 0a 0a 0a 0a 0a")),
            Message::EqUlt(EqUlt {
                preset: 0,
                ult: 2,
                bands: flat.clone()
            })
        );
        assert_eq!(
            parse(&hex("59 03 a0 02 06 0a 0a 0a 0a 0a 0a")),
            Message::EqUlt(EqUlt {
                preset: 0xa0,
                ult: 2,
                bands: flat
            })
        );
        match parse(&hex(
            "07 00 1e 10 ff 12 ff 13 ff 14 ff 15 ff 20 ff 23 ff 25 10 27 ff 32 13 53 04 69 0c \
             6b 03 70 01 93 ff f4 09 90 ff a1 05 f1 11 b2 16 b7 ff c1 ff c2 ff d1 08 d2 0b \
             d3 0d e3 06 e2 07 f9 14 fd 0f",
        )) {
            Message::SupportFunctions(f) => {
                assert_eq!(f.len(), 30);
                for x in [
                    func::BATTERY_LEVEL_INDICATOR,
                    func::PRESET_EQ_AND_ULT_MODE,
                    func::NC_ASM_DUAL_LEVEL,
                    0xa1,
                ] {
                    assert!(f.contains(&x));
                }
            }
            m => panic!("{m:?}"),
        }
        // truncated -> Other, no panic
        assert!(matches!(parse(&hex("67 17 01")), Message::Other(_)));
    }

    #[test]
    fn builders() {
        let ambient10 = NcAsm {
            enabled: true,
            ambient: true,
            focus_on_voice: false,
            level: 10,
        };
        assert_eq!(set_ncasm_dual(ambient10), hex("68 17 01 01 01 00 0a"));
        let off = NcAsm {
            enabled: false,
            ambient: false,
            focus_on_voice: false,
            level: 20,
        };
        assert_eq!(set_ncasm_dual(off), hex("68 17 01 00 00 00 14"));
        assert_eq!(
            set_eq_ult(&EqUlt {
                preset: 0,
                ult: 1,
                bands: vec![0x0a; 6]
            }),
            hex("58 03 00 01 06 0a 0a 0a 0a 0a 0a")
        );
        assert_eq!(set_volume(12), hex("a8 20 0c"));
        assert!(is_response_to(&get_eq(0), &hex("57 03 00")));
        assert!(is_response_to(&get_ncasm(0x17), &hex("67 17 01")));
        assert!(!is_response_to(&get_ncasm(0x17), &hex("67 30 02")));
    }
}
