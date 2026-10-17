//! In-memory fake headphones for UI development without hardware.

use super::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MOCK_ADDRESS: &str = "A8:E6:E8:00:00:01";

/// Shared fake device state, so reconnecting keeps settings.
#[derive(Debug)]
struct Inner {
    state: DeviceState,
    started: Instant,
    start_level: u8,
}

#[derive(Clone)]
pub struct MockBackend {
    inner: Arc<Mutex<Inner>>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: DeviceState {
                    model: Some("WH-ULT900N".into()),
                    firmware: Some("1.2.0 (mock)".into()),
                    battery: Some(Battery {
                        level: 78,
                        charging: false,
                    }),
                    volume: Some(45),
                    anc: Some(AncMode::NoiseCancelling),
                    ambient_level: Some(12),
                    focus_on_voice: Some(false),
                    ult: Some(UltMode::Off),
                },
                started: Instant::now(),
                start_level: 78,
            })),
        }
    }
}

impl MockBackend {
    fn info() -> DeviceInfo {
        DeviceInfo {
            name: "ULT WEAR (mock)".into(),
            address: MOCK_ADDRESS.into(),
            connected: true,
        }
    }
}

#[async_trait]
impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![
            Self::info(),
            DeviceInfo {
                name: "WH-1000XM5 (mock)".into(),
                address: "AC:80:0A:00:00:02".into(),
                connected: false,
            },
        ])
    }

    async fn connect(&self, address: &str) -> Result<Box<dyn DeviceHandle>> {
        if !address.eq_ignore_ascii_case(MOCK_ADDRESS) {
            return Err(format!("mock: device {address} is not connected"));
        }
        self.connect_first().await
    }

    async fn connect_first(&self) -> Result<Box<dyn DeviceHandle>> {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        Ok(Box::new(MockDevice {
            info: Self::info(),
            inner: self.inner.clone(),
        }))
    }
}

pub struct MockDevice {
    info: DeviceInfo,
    inner: Arc<Mutex<Inner>>,
}

#[async_trait]
impl DeviceHandle for MockDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn refresh(&mut self) -> Result<DeviceState> {
        let mut g = self.inner.lock().unwrap();
        // Slowly drain the battery: 1% per minute, never below 5%.
        let mins = (g.started.elapsed().as_secs() / 60) as u8;
        let level = g.start_level.saturating_sub(mins).max(5);
        if let Some(b) = g.state.battery.as_mut() {
            b.level = level;
        }
        Ok(g.state.clone())
    }

    async fn set_anc(
        &mut self,
        mode: AncMode,
        ambient_level: u8,
        focus_on_voice: bool,
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.state.anc = Some(mode);
        g.state.ambient_level = Some(ambient_level.min(20));
        g.state.focus_on_voice = Some(focus_on_voice);
        Ok(())
    }

    async fn set_ult(&mut self, mode: UltMode) -> Result<()> {
        self.inner.lock().unwrap().state.ult = Some(mode);
        Ok(())
    }

    async fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.inner.lock().unwrap().state.volume = Some(volume.min(100));
        Ok(())
    }

    async fn disconnect(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}
