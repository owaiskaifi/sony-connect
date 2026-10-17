//! Real headphones via the `sony-core` crate (feature `real`).
//! Written against the provisional API contract in crates/sony-core/src/lib.rs.

use super::*;

pub struct RealBackend;

fn err(e: sony_core::Error) -> String {
    e.to_string()
}

impl From<sony_core::DeviceInfo> for DeviceInfo {
    fn from(d: sony_core::DeviceInfo) -> Self {
        Self {
            name: d.name,
            address: d.address,
            connected: d.connected,
        }
    }
}

impl From<sony_core::AncMode> for AncMode {
    fn from(m: sony_core::AncMode) -> Self {
        match m {
            sony_core::AncMode::Off => Self::Off,
            sony_core::AncMode::NoiseCancelling => Self::NoiseCancelling,
            sony_core::AncMode::Ambient => Self::Ambient,
        }
    }
}

impl From<AncMode> for sony_core::AncMode {
    fn from(m: AncMode) -> Self {
        match m {
            AncMode::Off => Self::Off,
            AncMode::NoiseCancelling => Self::NoiseCancelling,
            AncMode::Ambient => Self::Ambient,
        }
    }
}

impl From<sony_core::UltMode> for UltMode {
    fn from(m: sony_core::UltMode) -> Self {
        match m {
            sony_core::UltMode::Off => Self::Off,
            sony_core::UltMode::Ult1 => Self::Ult1,
            sony_core::UltMode::Ult2 => Self::Ult2,
        }
    }
}

impl From<UltMode> for sony_core::UltMode {
    fn from(m: UltMode) -> Self {
        match m {
            UltMode::Off => Self::Off,
            UltMode::Ult1 => Self::Ult1,
            UltMode::Ult2 => Self::Ult2,
        }
    }
}

impl From<sony_core::DeviceState> for DeviceState {
    fn from(s: sony_core::DeviceState) -> Self {
        Self {
            model: s.model,
            firmware: s.firmware,
            battery: s.battery.map(|b| Battery {
                level: b.level,
                charging: b.charging,
            }),
            volume: s.volume,
            anc: s.anc.map(Into::into),
            ambient_level: s.ambient_level,
            focus_on_voice: s.focus_on_voice,
            ult: s.ult.map(Into::into),
        }
    }
}

#[async_trait]
impl Backend for RealBackend {
    fn name(&self) -> &'static str {
        "real"
    }

    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        Ok(sony_core::discover()
            .await
            .map_err(err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn connect(&self, address: &str) -> Result<Box<dyn DeviceHandle>> {
        let dev = sony_core::Device::connect(address).await.map_err(err)?;
        Ok(Box::new(RealDevice::new(dev)))
    }

    async fn connect_first(&self) -> Result<Box<dyn DeviceHandle>> {
        let dev = sony_core::Device::connect_first().await.map_err(err)?;
        Ok(Box::new(RealDevice::new(dev)))
    }
}

pub struct RealDevice {
    info: DeviceInfo,
    dev: sony_core::Device,
}

impl RealDevice {
    fn new(dev: sony_core::Device) -> Self {
        Self {
            info: dev.info().clone().into(),
            dev,
        }
    }
}

#[async_trait]
impl DeviceHandle for RealDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn refresh(&mut self) -> Result<DeviceState> {
        Ok(self.dev.refresh().await.map_err(err)?.into())
    }

    async fn set_anc(
        &mut self,
        mode: AncMode,
        ambient_level: u8,
        focus_on_voice: bool,
    ) -> Result<()> {
        self.dev
            .set_anc(mode.into(), ambient_level, focus_on_voice)
            .await
            .map_err(err)
    }

    async fn set_ult(&mut self, mode: UltMode) -> Result<()> {
        self.dev.set_ult(mode.into()).await.map_err(err)
    }

    async fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.dev.set_volume(volume).await.map_err(err)
    }

    async fn disconnect(self: Box<Self>) -> Result<()> {
        self.dev.disconnect().await.map_err(err)
    }
}
