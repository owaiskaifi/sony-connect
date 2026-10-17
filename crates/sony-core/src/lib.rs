//! sony-core: talk to Sony headphones (ULT WEAR first) over the Sony MDR
//! protocol on an RFCOMM channel.
//!
//! PUBLIC API CONTRACT (provisional until protocol findings in docs/PROTOCOL.md)
//! — the Tauri app depends on these items.
//! Do not rename or remove them; adding fields/variants is fine.

pub mod protocol;
pub mod transport;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bluetooth error: {0}")]
    Bluetooth(String),
    #[error("no supported Sony device found")]
    NotFound,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("timed out waiting for headphones")]
    Timeout,
    #[error("unsupported on this device: {0}")]
    Unsupported(String),
}

/// A paired Sony device visible to the system.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    /// "AA:BB:CC:DD:EE:FF"
    pub address: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AncMode {
    Off,
    NoiseCancelling,
    Ambient,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UltMode {
    Off,
    Ult1,
    Ult2,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Battery {
    /// 0-100
    pub level: u8,
    pub charging: bool,
}

/// Snapshot of everything the UI shows. `None` = not supported / not yet read.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DeviceState {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub battery: Option<Battery>,
    /// Device volume 0-30 as reported by the headphones (MDR), if available.
    pub volume: Option<u8>,
    pub anc: Option<AncMode>,
    /// Ambient sound level 0-20.
    pub ambient_level: Option<u8>,
    /// Ambient "focus on voice".
    pub focus_on_voice: Option<bool>,
    pub ult: Option<UltMode>,
}

pub use transport::Device;

/// List paired Sony devices (BlueZ on Linux).
pub async fn discover() -> Result<Vec<DeviceInfo>> {
    transport::discover().await
}

// `Device` (in transport) must provide:
//   pub async fn connect(address: &str) -> Result<Device>
//   pub async fn connect_first() -> Result<Device>        // first connected Sony device
//   pub fn info(&self) -> &DeviceInfo
//   pub async fn refresh(&mut self) -> Result<DeviceState> // query everything
//   pub async fn set_anc(&mut self, mode: AncMode, ambient_level: u8, focus_on_voice: bool) -> Result<()>
//   pub async fn set_ult(&mut self, mode: UltMode) -> Result<()>
//   pub async fn set_volume(&mut self, volume: u8) -> Result<()>
//   pub async fn disconnect(self) -> Result<()>
