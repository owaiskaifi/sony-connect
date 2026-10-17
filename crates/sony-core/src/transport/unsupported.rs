//! Placeholder for platforms without a backend yet. Everything returns
//! `Error::Unsupported` so the crate (and protocol tests) still build.

use std::time::Duration;

use crate::{AncMode, DeviceInfo, DeviceState, Error, Result, UltMode};

fn unsupported<T>() -> Result<T> {
    Err(Error::Unsupported(
        "no Bluetooth backend for this platform yet".into(),
    ))
}

pub async fn discover() -> Result<Vec<DeviceInfo>> {
    unsupported()
}

pub struct Device {
    info: DeviceInfo,
}

impl Device {
    pub async fn connect(_address: &str) -> Result<Device> {
        unsupported()
    }
    pub async fn connect_first() -> Result<Device> {
        unsupported()
    }
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }
    pub async fn refresh(&mut self) -> Result<DeviceState> {
        unsupported()
    }
    pub async fn poll(&mut self, _timeout: Duration) -> Result<Option<DeviceState>> {
        unsupported()
    }
    pub async fn set_anc(
        &mut self,
        _mode: AncMode,
        _ambient_level: u8,
        _focus_on_voice: bool,
    ) -> Result<()> {
        unsupported()
    }
    pub async fn set_ult(&mut self, _mode: UltMode) -> Result<()> {
        unsupported()
    }
    pub async fn set_volume(&mut self, _volume: u8) -> Result<()> {
        unsupported()
    }
    pub async fn disconnect(self) -> Result<()> {
        Ok(())
    }
}
