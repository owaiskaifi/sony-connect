//! Device backend abstraction.
//!
//! The app talks to headphones only through the [`Backend`] / [`DeviceHandle`]
//! traits below, which mirror the `sony-core` public API contract. Two
//! implementations exist:
//!
//! * `mock` (feature `mock`, default) — an in-memory fake device.
//! * `real` (feature `real`) — wraps `sony_core::Device`.
//!
//! If both features are enabled, `real` wins.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(feature = "mock")]
pub mod mock;
#[cfg(feature = "real")]
pub mod real;

#[cfg(not(any(feature = "mock", feature = "real")))]
compile_error!("enable either the `mock` or the `real` feature of sony-connect");

pub type Result<T> = std::result::Result<T, String>;

/// A paired device visible to the system (mirrors `sony_core::DeviceInfo`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
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

/// Snapshot of everything the UI shows. `None` = unsupported / not read yet.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DeviceState {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub battery: Option<Battery>,
    /// Volume 0-100 (percent).
    pub volume: Option<u8>,
    pub anc: Option<AncMode>,
    /// Ambient sound level 0-20.
    pub ambient_level: Option<u8>,
    pub focus_on_voice: Option<bool>,
    pub ult: Option<UltMode>,
}

/// A connected device (mirrors the methods of `sony_core::Device`).
#[async_trait]
pub trait DeviceHandle: Send {
    fn info(&self) -> &DeviceInfo;
    async fn refresh(&mut self) -> Result<DeviceState>;
    async fn set_anc(
        &mut self,
        mode: AncMode,
        ambient_level: u8,
        focus_on_voice: bool,
    ) -> Result<()>;
    async fn set_ult(&mut self, mode: UltMode) -> Result<()>;
    async fn set_volume(&mut self, volume: u8) -> Result<()>;
    async fn disconnect(self: Box<Self>) -> Result<()>;
}

/// Entry points (mirrors `sony_core::discover`, `Device::connect`, `Device::connect_first`).
#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn discover(&self) -> Result<Vec<DeviceInfo>>;
    async fn connect(&self, address: &str) -> Result<Box<dyn DeviceHandle>>;
    async fn connect_first(&self) -> Result<Box<dyn DeviceHandle>>;
}

/// The backend selected at compile time.
pub fn default_backend() -> Box<dyn Backend> {
    #[cfg(feature = "real")]
    {
        Box::new(real::RealBackend)
    }
    #[cfg(all(feature = "mock", not(feature = "real")))]
    {
        Box::new(mock::MockBackend::default())
    }
}
