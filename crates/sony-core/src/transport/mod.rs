//! Transport layer.
//!
//! The platform seam is this module: a platform backend provides a byte
//! [`Link`] (RFCOMM on Linux) plus OS-level helpers (device discovery,
//! system battery/volume), and exports `Device` + `discover()`. Everything
//! protocol-related (sequence numbers, ACKs, retries, request matching, state
//! cache) lives in the platform-independent [`Session`] below, so a future
//! backend only has to supply a `Link`.

pub mod sdp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{discover, Device};

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::{discover, Device};

use std::time::Duration;
use tokio::time::Instant;

use crate::protocol::{self, func, DataType, EqUlt, Frame, Message, NcAsm};
use crate::{Error, Result};

/// Byte stream to the headphones (one RFCOMM connection).
#[allow(async_fn_in_trait)]
pub(crate) trait Link {
    async fn send(&mut self, bytes: &[u8]) -> Result<()>;
    /// Read some bytes; `Ok(0)` means the peer closed the connection.
    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize>;
}

const ACK_TIMEOUT: Duration = Duration::from_millis(1000);
const ACK_RETRIES: usize = 3;
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(2500);
const NOTIFY_WAIT: Duration = Duration::from_millis(800);

/// Values last reported by the headphones over MDR.
#[derive(Debug, Clone, Default)]
pub(crate) struct MdrCache {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub battery: Option<(u8, bool)>,
    pub ncasm: Option<NcAsm>,
    pub eq: Option<EqUlt>,
    /// 0..=30
    pub volume: Option<u8>,
}

impl MdrCache {
    fn apply(&mut self, m: &Message) {
        match m {
            Message::ModelName(s) => self.model = Some(s.clone()),
            Message::FwVersion(s) => self.firmware = Some(s.clone()),
            Message::Battery { level, charging } => self.battery = Some((*level, *charging)),
            Message::NcAsm(n) => self.ncasm = Some(*n),
            Message::EqUlt(e) => self.eq = Some(e.clone()),
            Message::Volume(v) => self.volume = Some(*v),
            _ => {}
        }
    }
}

/// MDR v2 session over a [`Link`].
pub(crate) struct Session<L: Link> {
    link: L,
    decoder: protocol::Decoder,
    seq: u8,
    pub cache: MdrCache,
    /// Table-1 support function ids.
    pub support: Vec<u8>,
    pub debug: bool,
}

impl<L: Link> Session<L> {
    pub fn new(link: L) -> Self {
        Self {
            link,
            decoder: protocol::Decoder::new(),
            seq: 0,
            cache: MdrCache::default(),
            support: Vec::new(),
            debug: std::env::var_os("SONY_CONNECT_DEBUG").is_some(),
        }
    }

    pub fn supports(&self, f: u8) -> bool {
        self.support.contains(&f)
    }

    /// Next frame before `deadline`, or `None` on timeout.
    async fn read_frame(&mut self, deadline: Instant) -> Result<Option<Frame>> {
        let mut buf = [0u8; 2048];
        loop {
            if let Some(f) = self.decoder.next_frame() {
                if self.debug {
                    eprintln!(
                        "<< type={:#04x} seq={} {:02x?}",
                        f.data_type, f.seq, f.payload
                    );
                }
                return Ok(Some(f));
            }
            let n = match tokio::time::timeout_at(deadline, self.link.recv(&mut buf)).await {
                Err(_) => return Ok(None),
                Ok(r) => r?,
            };
            if n == 0 {
                return Err(Error::Bluetooth("headphones closed the connection".into()));
            }
            self.decoder.push(&buf[..n]);
        }
    }

    /// ACK + parse + cache a non-ACK frame. Returns the table-1 payload.
    async fn handle_data(&mut self, f: &Frame) -> Result<Option<Vec<u8>>> {
        self.link.send(&protocol::encode_ack(f.seq)).await?;
        if f.data_type == DataType::DataMdr as u8 {
            let m = protocol::parse(&f.payload);
            self.cache.apply(&m);
            return Ok(Some(f.payload.clone()));
        }
        Ok(None)
    }

    /// Send a payload and wait for its ACK (with retries). Any data frames that
    /// arrive meanwhile are handled and returned (a response can beat the ACK).
    async fn send_cmd(&mut self, table: DataType, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut seen = Vec::new();
        let frame = protocol::encode(table as u8, self.seq, payload);
        for _ in 0..ACK_RETRIES {
            if self.debug {
                eprintln!(
                    ">> type={:#04x} seq={} {:02x?}",
                    table as u8, self.seq, payload
                );
            }
            self.link.send(&frame).await?;
            let deadline = Instant::now() + ACK_TIMEOUT;
            while let Some(f) = self.read_frame(deadline).await? {
                if f.is_ack() {
                    if f.seq == 1 - self.seq {
                        self.seq = f.seq;
                        return Ok(seen);
                    }
                    continue; // stale ACK
                }
                if let Some(p) = self.handle_data(&f).await? {
                    seen.push(p);
                }
            }
        }
        Err(Error::Timeout)
    }

    /// Send a GET and wait for the matching RET.
    pub async fn request(&mut self, payload: &[u8]) -> Result<Message> {
        self.request_table(DataType::DataMdr, payload).await
    }

    pub async fn request_table(&mut self, table: DataType, payload: &[u8]) -> Result<Message> {
        let seen = self.send_cmd(table, payload).await?;
        if let Some(p) = seen.iter().find(|p| protocol::is_response_to(payload, p)) {
            return Ok(protocol::parse(p));
        }
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        while let Some(f) = self.read_frame(deadline).await? {
            if f.is_ack() {
                continue;
            }
            if f.data_type == table as u8 && protocol::is_response_to(payload, &f.payload) {
                self.handle_data(&f).await?;
                return Ok(protocol::parse(&f.payload));
            }
            self.handle_data(&f).await?;
        }
        Err(Error::Timeout)
    }

    /// Send a SET; wait for ACK and (best effort) for a NTFY with command `notify`.
    pub async fn command(&mut self, payload: &[u8], notify: u8) -> Result<Option<Message>> {
        let seen = self.send_cmd(DataType::DataMdr, payload).await?;
        if let Some(p) = seen.iter().find(|p| p.first() == Some(&notify)) {
            return Ok(Some(protocol::parse(p)));
        }
        let deadline = Instant::now() + NOTIFY_WAIT;
        while let Some(f) = self.read_frame(deadline).await? {
            if f.is_ack() {
                continue;
            }
            if let Some(p) = self.handle_data(&f).await? {
                if p.first() == Some(&notify) {
                    return Ok(Some(protocol::parse(&p)));
                }
            }
        }
        Ok(None)
    }

    /// Process notifications that arrive within `timeout`. Returns how many
    /// table-1 messages were handled.
    pub async fn pump(&mut self, timeout: Duration) -> Result<usize> {
        let deadline = Instant::now() + timeout;
        let mut n = 0;
        while let Some(f) = self.read_frame(deadline).await? {
            if !f.is_ack() && self.handle_data(&f).await?.is_some() {
                n += 1;
            }
        }
        Ok(n)
    }

    // ---- high level -------------------------------------------------------

    /// Protocol info, support functions, model and firmware.
    pub async fn init(&mut self) -> Result<()> {
        match self.request(&protocol::get_protocol_info()).await? {
            Message::ProtocolInfo { table1: true, .. } => {}
            Message::ProtocolInfo { .. } => {
                return Err(Error::Unsupported(
                    "device does not support MDR v2 table 1".into(),
                ))
            }
            m => {
                return Err(Error::Protocol(format!(
                    "unexpected protocol info reply: {m:?}"
                )))
            }
        }
        if let Message::SupportFunctions(f) =
            self.request(&protocol::get_support_function()).await?
        {
            self.support = f;
        }
        let _ = self
            .request(&protocol::get_device_info(protocol::DEVICE_INFO_MODEL_NAME))
            .await;
        let _ = self
            .request(&protocol::get_device_info(protocol::DEVICE_INFO_FW_VERSION))
            .await;
        Ok(())
    }

    fn ncasm_type(&self) -> Option<u8> {
        if self.supports(func::NC_ASM_DUAL_LEVEL) {
            Some(protocol::ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS)
        } else if self.supports(func::NC_ASM_DUAL_LEVEL_NOISE_ADAPTATION) {
            Some(protocol::ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS_NA)
        } else if self.supports(func::AMBIENT_SOUND_MODE_LEVEL_ADJUSTMENT) {
            Some(protocol::ncasm::ASM_SEAMLESS)
        } else {
            None
        }
    }

    fn has_volume(&self) -> bool {
        [
            func::PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT,
            func::PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT_AND_MUTE,
            func::PLAYBACK_CONTROLLER_WITH_CALL_VOLUME_ADJUSTMENT_AND_FUNCTION_CHANGE,
            func::PLAYBACK_CONTROLLER_WITH_FUNCTION_CHANGE,
        ]
        .iter()
        .any(|f| self.supports(*f))
    }

    /// Query battery, NC/ASM, EQ/ULT and volume (whatever is supported).
    /// Individual failures leave the cached value untouched.
    pub async fn refresh(&mut self) -> Result<()> {
        let battery = if self.supports(func::BATTERY_LEVEL_INDICATOR) {
            Some(protocol::power::BATTERY)
        } else if self.supports(func::BATTERY_LEVEL_WITH_THRESHOLD) {
            Some(protocol::power::BATTERY_WITH_THRESHOLD)
        } else if self.supports(func::LEFT_RIGHT_BATTERY_LEVEL_INDICATOR) {
            Some(protocol::power::LEFT_RIGHT_BATTERY)
        } else {
            None
        };
        if let Some(t) = battery {
            soft(self.request(&protocol::get_battery(t)).await)?;
        }
        if let Some(t) = self.ncasm_type() {
            soft(self.request(&protocol::get_ncasm(t)).await)?;
        }
        if self.supports(func::PRESET_EQ_AND_ULT_MODE) {
            soft(
                self.request(&protocol::get_eq(protocol::eq::PRESET_EQ_AND_ULT_MODE))
                    .await,
            )?;
        }
        if self.has_volume() {
            soft(self.request(&protocol::get_volume()).await)?;
        }
        Ok(())
    }

    pub async fn set_ncasm(&mut self, s: NcAsm) -> Result<()> {
        let payload = match self.ncasm_type() {
            Some(protocol::ncasm::ASM_SEAMLESS) => protocol::set_ncasm_asm_seamless(s),
            Some(protocol::ncasm::DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS) => {
                protocol::set_ncasm_dual(s)
            }
            _ => {
                return Err(Error::Unsupported(
                    "noise cancelling / ambient sound control".into(),
                ))
            }
        };
        if let Some(Message::NcAsm(n)) = self
            .command(&payload, protocol::cmd::NCASM_NTFY_PARAM)
            .await?
        {
            self.cache.ncasm = Some(n);
        } else {
            self.cache.ncasm = Some(s);
        }
        Ok(())
    }

    /// Read-modify-write of the EQ+ULT parameter: only the ULT byte changes.
    pub async fn set_ult(&mut self, ult: u8) -> Result<()> {
        if !self.supports(func::PRESET_EQ_AND_ULT_MODE) {
            return Err(Error::Unsupported("ULT mode".into()));
        }
        let cur = match self
            .request(&protocol::get_eq(protocol::eq::PRESET_EQ_AND_ULT_MODE))
            .await?
        {
            Message::EqUlt(e) => e,
            m => return Err(Error::Protocol(format!("unexpected EQ reply: {m:?}"))),
        };
        let new = EqUlt { ult, ..cur };
        self.command(&protocol::set_eq_ult(&new), protocol::cmd::EQEBB_NTFY_PARAM)
            .await?;
        // Confirm what the device actually stored.
        self.request(&protocol::get_eq(protocol::eq::PRESET_EQ_AND_ULT_MODE))
            .await?;
        match &self.cache.eq {
            Some(e) if e.ult == ult => Ok(()),
            e => Err(Error::Protocol(format!(
                "ULT mode not applied (device reports {e:?})"
            ))),
        }
    }

    /// `v` in device units 0..=30.
    pub async fn set_volume(&mut self, v: u8) -> Result<()> {
        if !self.has_volume() {
            return Err(Error::Unsupported("volume".into()));
        }
        let v = v.min(protocol::MDR_VOLUME_MAX);
        self.command(&protocol::set_volume(v), protocol::cmd::PLAY_NTFY_PARAM)
            .await?;
        self.cache.volume = Some(v);
        Ok(())
    }
}

/// Timeouts on individual queries are tolerated; link errors are not.
fn soft(r: Result<Message>) -> Result<()> {
    match r {
        Ok(_) | Err(Error::Timeout) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Map MDR NC/ASM state to the public enum.
pub(crate) fn anc_mode(n: &NcAsm) -> crate::AncMode {
    match (n.enabled, n.ambient) {
        (false, _) => crate::AncMode::Off,
        (true, false) => crate::AncMode::NoiseCancelling,
        (true, true) => crate::AncMode::Ambient,
    }
}

pub(crate) fn ult_mode(e: &EqUlt) -> Option<crate::UltMode> {
    Some(match e.ult {
        0 => crate::UltMode::Off,
        1 => crate::UltMode::Ult1,
        2 => crate::UltMode::Ult2,
        _ => return None,
    })
}

pub(crate) fn ult_byte(m: crate::UltMode) -> u8 {
    match m {
        crate::UltMode::Off => 0,
        crate::UltMode::Ult1 => 1,
        crate::UltMode::Ult2 => 2,
    }
}

/// Percent (0-100) <-> device scale helpers.
pub(crate) fn to_percent(v: u32, max: u32) -> u8 {
    ((v.min(max) * 100 + max / 2) / max) as u8
}

pub(crate) fn from_percent(p: u8, max: u32) -> u32 {
    (p.min(100) as u32 * max + 50) / 100
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn percent_scaling() {
        assert_eq!(to_percent(51, 127), 40);
        assert_eq!(to_percent(12, 30), 40);
        assert_eq!(to_percent(127, 127), 100);
        assert_eq!(from_percent(40, 30), 12);
        assert_eq!(from_percent(100, 127), 127);
        assert_eq!(from_percent(0, 127), 0);
    }

    /// Scripted fake headphones: for each frame we send, reply with canned bytes.
    struct Fake {
        replies: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        pending: VecDeque<u8>,
    }

    impl Link for Fake {
        async fn send(&mut self, bytes: &[u8]) -> Result<()> {
            self.sent.push(bytes.to_vec());
            // only data frames (not our ACKs) trigger a scripted reply
            if bytes.get(1) != Some(&0x01) {
                if let Some(r) = self.replies.pop_front() {
                    self.pending.extend(r);
                }
            }
            Ok(())
        }
        async fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
            if self.pending.is_empty() {
                std::future::pending::<()>().await;
            }
            let n = self.pending.len().min(buf.len());
            for b in buf.iter_mut().take(n) {
                *b = self.pending.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn request_with_ack_and_response() {
        let mut reply = protocol::encode(0x01, 1, &[]);
        reply.extend(protocol::encode(0x0c, 0, &hex("01 00 03 00 20 02 00 00")));
        let mut s = Session::new(Fake {
            replies: [reply].into(),
            sent: vec![],
            pending: Default::default(),
        });
        let m = s.request(&protocol::get_protocol_info()).await.unwrap();
        assert!(matches!(m, Message::ProtocolInfo { table1: true, .. }));
        assert_eq!(s.seq, 1);
        // we ACKed the device's data frame (seq 0) with seq 1
        assert_eq!(s.link.sent[1], protocol::encode_ack(0));
    }

    #[tokio::test]
    async fn set_ult_echoes_preset_and_bands() {
        let ack = |seq| protocol::encode(0x01, seq, &[]);
        let data = |seq, p: &str| protocol::encode(0x0c, seq, &hex(p));
        let replies = [
            [ack(1), data(0, "57 03 a0 02 06 01 02 03 04 05 06")].concat(),
            [ack(0), data(1, "59 03 a0 01 06 01 02 03 04 05 06")].concat(),
            [ack(1), data(0, "57 03 a0 01 06 01 02 03 04 05 06")].concat(),
        ];
        let mut s = Session::new(Fake {
            replies: replies.into(),
            sent: vec![],
            pending: Default::default(),
        });
        s.support = vec![func::PRESET_EQ_AND_ULT_MODE];
        s.set_ult(1).await.unwrap();
        let set = protocol::encode(0x0c, 1, &hex("58 03 a0 01 06 01 02 03 04 05 06"));
        assert!(s.link.sent.contains(&set));
    }
}
