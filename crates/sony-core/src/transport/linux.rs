//! Linux backend: raw BlueZ sockets (SDP over L2CAP, MDR over RFCOMM) via libc,
//! and BlueZ D-Bus (zbus) for discovery, battery and A2DP volume.
//!
//! Channel discovery mirrors the reference (mos9527/SonyHeadphonesClient
//! `libmdr-bt/src/Linux/ConnectionLinux.cpp`): an SDP ServiceSearchAttribute
//! query for the MDR service UUID, then the RFCOMM port from the
//! ProtocolDescriptorList. Override with `SONY_CONNECT_RFCOMM_CHANNEL=<n>`.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use tokio::io::unix::AsyncFd;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{anc_mode, from_percent, sdp, to_percent, ult_byte, ult_mode, Link, Session};
use crate::protocol::{self, NcAsm};
use crate::{AncMode, Battery, DeviceInfo, DeviceState, Error, Result, UltMode};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_L2CAP: libc::c_int = 0;
const BTPROTO_RFCOMM: libc::c_int = 3;
const SOL_RFCOMM: libc::c_int = 18;
const RFCOMM_LM: libc::c_int = 0x03;
const RFCOMM_LM_AUTH: u32 = 0x0002;
const RFCOMM_LM_ENCRYPT: u32 = 0x0004;
const SDP_PSM: u16 = 1;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

/// Sony's Bluetooth SIG / USB vendor id as it appears in BlueZ Modalias.
const SONY_MODALIAS: &str = "v054C";

#[repr(C)]
struct SockaddrRc {
    rc_family: libc::sa_family_t,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

#[repr(C)]
struct SockaddrL2 {
    l2_family: libc::sa_family_t,
    l2_psm: u16,
    l2_bdaddr: [u8; 6],
    l2_cid: u16,
    l2_bdaddr_type: u8,
}

fn bt_err(ctx: &str, e: io::Error) -> Error {
    Error::Bluetooth(format!("{ctx}: {e}"))
}

/// "A8:E6:E8:6C:6B:B1" -> bdaddr_t (little-endian byte order).
fn parse_bdaddr(addr: &str) -> Result<[u8; 6]> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 6 {
        return Err(Error::Bluetooth(format!("invalid address {addr:?}")));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[5 - i] = u8::from_str_radix(p, 16)
            .map_err(|_| Error::Bluetooth(format!("invalid address {addr:?}")))?;
    }
    Ok(out)
}

fn socket(ty: libc::c_int, proto: libc::c_int) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(AF_BLUETOOTH, ty | libc::SOCK_CLOEXEC, proto) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn set_timeouts(fd: &OwnedFd, t: Duration) -> io::Result<()> {
    let tv = libc::timeval {
        tv_sec: t.as_secs() as _,
        tv_usec: t.subsec_micros() as _,
    };
    for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        let r = unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                opt,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn connect_raw<T>(fd: &OwnedFd, addr: &T) -> io::Result<()> {
    let r = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            addr as *const T as *const libc::sockaddr,
            std::mem::size_of::<T>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Blocking SDP query: RFCOMM channel advertising `uuid` on `bdaddr`.
fn sdp_rfcomm_channel(bdaddr: [u8; 6], uuid: &str) -> io::Result<Option<u8>> {
    let uuid = sdp::parse_uuid128(uuid).ok_or_else(|| io::Error::other("bad uuid"))?;
    let fd = socket(libc::SOCK_SEQPACKET, BTPROTO_L2CAP)?;
    set_timeouts(&fd, SOCKET_TIMEOUT)?;
    let addr = SockaddrL2 {
        l2_family: AF_BLUETOOTH as _,
        l2_psm: SDP_PSM.to_le(),
        l2_bdaddr: bdaddr,
        l2_cid: 0,
        l2_bdaddr_type: 0, // BDADDR_BREDR
    };
    connect_raw(&fd, &addr)?;
    let mut lists = Vec::new();
    let mut cont: Vec<u8> = Vec::new();
    for tid in 1..=16u16 {
        let req = sdp::search_attr_request(tid, &uuid, &cont);
        let n = unsafe { libc::send(fd.as_raw_fd(), req.as_ptr() as *const _, req.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; 4096];
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        let (chunk, next) = sdp::parse_search_attr_response(&buf)
            .ok_or_else(|| io::Error::other(format!("unexpected SDP response {buf:02x?}")))?;
        lists.extend(chunk);
        if next.is_empty() {
            return Ok(sdp::rfcomm_channel(&lists));
        }
        cont = next;
    }
    Err(io::Error::other("SDP continuation did not terminate"))
}

/// Blocking RFCOMM connect; returns a non-blocking fd.
fn rfcomm_connect(bdaddr: [u8; 6], channel: u8) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_STREAM, BTPROTO_RFCOMM)?;
    set_timeouts(&fd, SOCKET_TIMEOUT)?;
    let lm: u32 = RFCOMM_LM_AUTH | RFCOMM_LM_ENCRYPT;
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            SOL_RFCOMM,
            RFCOMM_LM,
            &lm as *const _ as *const libc::c_void,
            std::mem::size_of::<u32>() as libc::socklen_t,
        );
    }
    let addr = SockaddrRc {
        rc_family: AF_BLUETOOTH as _,
        rc_bdaddr: bdaddr,
        rc_channel: channel,
    };
    connect_raw(&fd, &addr)?;
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

pub(crate) struct RfcommLink {
    fd: AsyncFd<OwnedFd>,
}

impl Link for RfcommLink {
    async fn send(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            let mut guard = self.fd.writable().await.map_err(|e| bt_err("rfcomm", e))?;
            let r = guard.try_io(|fd| {
                let n = unsafe {
                    libc::send(
                        fd.as_raw_fd(),
                        bytes.as_ptr() as *const _,
                        bytes.len(),
                        libc::MSG_NOSIGNAL,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match r {
                Ok(Ok(n)) => bytes = &bytes[n..],
                Ok(Err(e)) => return Err(bt_err("rfcomm send", e)),
                Err(_would_block) => continue,
            }
        }
        Ok(())
    }

    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
        loop {
            let mut guard = self.fd.readable().await.map_err(|e| bt_err("rfcomm", e))?;
            let r = guard.try_io(|fd| {
                let n =
                    unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), 0) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match r {
                Ok(Ok(n)) => return Ok(n),
                // ECONNRESET etc. when the headset goes away
                Ok(Err(e)) => return Err(bt_err("rfcomm recv", e)),
                Err(_would_block) => continue,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BlueZ D-Bus
// ---------------------------------------------------------------------------

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

async fn system_bus() -> Result<zbus::Connection> {
    zbus::Connection::system()
        .await
        .map_err(|e| Error::Bluetooth(format!("cannot connect to the system D-Bus: {e}")))
}

async fn managed_objects(conn: &zbus::Connection) -> Result<Objects> {
    let proxy = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")
        .and_then(|b| b.path("/"))
        .map_err(|e| Error::Bluetooth(e.to_string()))?
        .build()
        .await
        .map_err(|e| Error::Bluetooth(e.to_string()))?;
    let objs = proxy.get_managed_objects().await.map_err(|e| {
        Error::Bluetooth(format!(
            "BlueZ GetManagedObjects failed (is bluetoothd running?): {e}"
        ))
    })?;
    Ok(objs
        .into_iter()
        .map(|(p, ifaces)| {
            (
                p,
                ifaces
                    .into_iter()
                    .map(|(i, props)| (i.to_string(), props))
                    .collect(),
            )
        })
        .collect())
}

fn prop_str(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(&**v).ok())
        .map(str::to_owned)
}

fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| bool::try_from(&**v).ok())
}

fn prop_strs(props: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default()
}

fn is_sony(props: &HashMap<String, OwnedValue>) -> bool {
    if let Some(m) = prop_str(props, "Modalias") {
        if m.to_ascii_uppercase()
            .contains(&SONY_MODALIAS.to_ascii_uppercase())
        {
            return true;
        }
    }
    let uuids = prop_strs(props, "UUIDs");
    if uuids.iter().any(|u| {
        u.eq_ignore_ascii_case(protocol::SERVICE_UUID_V2)
            || u.eq_ignore_ascii_case(protocol::SERVICE_UUID_V1)
    }) {
        return true;
    }
    let name = prop_str(props, "Name")
        .unwrap_or_default()
        .to_ascii_uppercase();
    ["WH-", "WF-", "WI-", "ULT WEAR", "LINKBUDS", "INZONE"]
        .iter()
        .any(|p| name.starts_with(p))
}

struct BluezDevice {
    path: OwnedObjectPath,
    info: DeviceInfo,
}

fn sony_devices(objs: &Objects) -> Vec<BluezDevice> {
    let mut out: Vec<BluezDevice> = objs
        .iter()
        .filter_map(|(path, ifaces)| {
            let props = ifaces.get("org.bluez.Device1")?;
            if !prop_bool(props, "Paired").unwrap_or(false) || !is_sony(props) {
                return None;
            }
            let name = prop_str(props, "Alias")
                .or_else(|| prop_str(props, "Name"))
                .unwrap_or_default();
            Some(BluezDevice {
                path: path.clone(),
                info: DeviceInfo {
                    name,
                    address: prop_str(props, "Address")?.to_ascii_uppercase(),
                    connected: prop_bool(props, "Connected").unwrap_or(false),
                },
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.info
            .connected
            .cmp(&a.info.connected)
            .then(a.info.name.cmp(&b.info.name))
    });
    out
}

/// List paired Sony devices known to BlueZ (connected ones first).
pub async fn discover() -> Result<Vec<DeviceInfo>> {
    let conn = system_bus().await?;
    Ok(sony_devices(&managed_objects(&conn).await?)
        .into_iter()
        .map(|d| d.info)
        .collect())
}

/// BlueZ-side battery percentage and A2DP transport volume (0-127).
struct SystemAudio {
    battery: Option<u8>,
    transport: Option<(OwnedObjectPath, u16)>,
}

async fn system_audio(conn: &zbus::Connection, dev: &OwnedObjectPath) -> Result<SystemAudio> {
    let objs = managed_objects(conn).await?;
    let battery = objs
        .get(dev)
        .and_then(|i| i.get("org.bluez.Battery1"))
        .and_then(|p| p.get("Percentage"))
        .and_then(|v| u8::try_from(&**v).ok());
    let transport = objs.iter().find_map(|(path, ifaces)| {
        let t = ifaces.get("org.bluez.MediaTransport1")?;
        let owner = t
            .get("Device")
            .and_then(|v| zbus::zvariant::ObjectPath::try_from(&**v).ok())?;
        if owner.as_str() != dev.as_str() {
            return None;
        }
        let vol = t.get("Volume").and_then(|v| u16::try_from(&**v).ok())?;
        Some((path.clone(), vol))
    });
    Ok(SystemAudio { battery, transport })
}

async fn set_transport_volume(
    conn: &zbus::Connection,
    path: &OwnedObjectPath,
    v: u16,
) -> Result<()> {
    let proxy = zbus::fdo::PropertiesProxy::builder(conn)
        .destination("org.bluez")
        .and_then(|b| b.path(path.clone()))
        .map_err(|e| Error::Bluetooth(e.to_string()))?
        .build()
        .await
        .map_err(|e| Error::Bluetooth(e.to_string()))?;
    let iface = zbus::names::InterfaceName::from_static_str_unchecked("org.bluez.MediaTransport1");
    proxy
        .set(iface, "Volume", Value::from(v))
        .await
        .map_err(|e| Error::Bluetooth(format!("setting A2DP volume failed: {e}")))
}

const BLUEZ_VOLUME_MAX: u32 = 127;

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

/// A connected Sony headset (MDR v2 over RFCOMM + BlueZ D-Bus).
pub struct Device {
    info: DeviceInfo,
    path: OwnedObjectPath,
    bus: zbus::Connection,
    session: Session<RfcommLink>,
}

impl Device {
    /// Connect to a paired Sony device by address ("AA:BB:CC:DD:EE:FF").
    /// The device must already be connected at the Bluetooth level.
    pub async fn connect(address: &str) -> Result<Device> {
        let bus = system_bus().await?;
        let objs = managed_objects(&bus).await?;
        let want = address.to_ascii_uppercase();
        let dev = sony_devices(&objs)
            .into_iter()
            .find(|d| d.info.address == want)
            .ok_or(Error::NotFound)?;
        if !dev.info.connected {
            return Err(Error::Bluetooth(format!(
                "{} ({}) is paired but not connected",
                dev.info.name, want
            )));
        }
        let bdaddr = parse_bdaddr(&want)?;

        let channel = match std::env::var("SONY_CONNECT_RFCOMM_CHANNEL").ok().and_then(|s| s.parse().ok()) {
            Some(c) => c,
            None => tokio::task::spawn_blocking(move || sdp_rfcomm_channel(bdaddr, protocol::SERVICE_UUID_V2))
                .await
                .map_err(|e| Error::Bluetooth(e.to_string()))?
                .map_err(|e| bt_err("SDP query failed", e))?
                .ok_or_else(|| {
                    Error::Unsupported(
                        "device does not advertise the MDR v2 service (older MDR v1 devices are not supported yet)"
                            .into(),
                    )
                })?,
        };

        let fd = tokio::task::spawn_blocking(move || rfcomm_connect(bdaddr, channel))
            .await
            .map_err(|e| Error::Bluetooth(e.to_string()))?
            .map_err(|e| bt_err(&format!("RFCOMM connect (channel {channel})"), e))?;
        // SAFETY: we own `fd` (OwnedFd) and nothing else registers or closes it.
        let fd = unsafe { AsyncFd::register(fd) }
            .map_err(|e| Error::Bluetooth(format!("rfcomm: cannot register with tokio: {e}")))?;
        let mut session = Session::new(RfcommLink { fd });
        session.init().await?;
        Ok(Device {
            info: dev.info,
            path: dev.path,
            bus,
            session,
        })
    }

    /// Connect to the first connected Sony device.
    pub async fn connect_first() -> Result<Device> {
        let dev = discover()
            .await?
            .into_iter()
            .find(|d| d.connected)
            .ok_or(Error::NotFound)?;
        Self::connect(&dev.address).await
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// State from cached values (no I/O). See [`Device::refresh`].
    fn state(&self, sys: Option<&SystemAudio>) -> DeviceState {
        let c = &self.session.cache;
        let charging = c.battery.map(|b| b.1).unwrap_or(false);
        // MDR reports per-percent; BlueZ moves in coarse steps, so it is only a fallback.
        let battery = c
            .battery
            .map(|(level, charging)| Battery { level, charging })
            .or_else(|| {
                sys.and_then(|s| s.battery)
                    .map(|level| Battery { level, charging })
            });
        let volume = sys
            .and_then(|s| s.transport.as_ref())
            .map(|(_, v)| to_percent(*v as u32, BLUEZ_VOLUME_MAX))
            .or_else(|| {
                c.volume
                    .map(|v| to_percent(v as u32, protocol::MDR_VOLUME_MAX as u32))
            });
        DeviceState {
            model: c.model.clone(),
            firmware: c.firmware.clone(),
            battery,
            volume,
            anc: c.ncasm.as_ref().map(anc_mode),
            ambient_level: c.ncasm.map(|n| n.level),
            focus_on_voice: c.ncasm.map(|n| n.focus_on_voice),
            ult: c.eq.as_ref().and_then(ult_mode),
        }
    }

    /// Query everything. Battery level and volume come from BlueZ when
    /// available (MDR otherwise); charging state always comes from MDR.
    pub async fn refresh(&mut self) -> Result<DeviceState> {
        self.session.refresh().await?;
        let sys = system_audio(&self.bus, &self.path).await.ok();
        Ok(self.state(sys.as_ref()))
    }

    /// Wait up to `timeout` for notifications from the headphones (e.g. the user
    /// pressed a button), then return the updated state without querying.
    /// Returns `None` if nothing arrived.
    pub async fn poll(&mut self, timeout: Duration) -> Result<Option<DeviceState>> {
        if self.session.pump(timeout).await? == 0 {
            return Ok(None);
        }
        let sys = system_audio(&self.bus, &self.path).await.ok();
        Ok(Some(self.state(sys.as_ref())))
    }

    /// `ambient_level` 0-20 (clamped). `ambient_level` and `focus_on_voice` are
    /// only applied in `Ambient` mode: the ULT WEAR ignores them (keeps its
    /// stored values) when the mode is `NoiseCancelling`; `Off` keeps them too.
    pub async fn set_anc(
        &mut self,
        mode: AncMode,
        ambient_level: u8,
        focus_on_voice: bool,
    ) -> Result<()> {
        let s = NcAsm {
            enabled: mode != AncMode::Off,
            ambient: mode == AncMode::Ambient,
            focus_on_voice,
            level: ambient_level.min(20),
        };
        self.session.set_ncasm(s).await
    }

    pub async fn set_ult(&mut self, mode: UltMode) -> Result<()> {
        self.session.set_ult(ult_byte(mode)).await
    }

    /// Volume in percent 0-100. Uses the BlueZ A2DP transport volume (which
    /// keeps the system mixer in sync) when an A2DP stream exists, otherwise
    /// the MDR volume command.
    pub async fn set_volume(&mut self, volume: u8) -> Result<()> {
        let volume = volume.min(100);
        let sys = system_audio(&self.bus, &self.path).await.ok();
        if let Some((path, _)) = sys.and_then(|s| s.transport) {
            let v = from_percent(volume, BLUEZ_VOLUME_MAX) as u16;
            if set_transport_volume(&self.bus, &path, v).await.is_ok() {
                return Ok(());
            }
        }
        let v = from_percent(volume, protocol::MDR_VOLUME_MAX as u32) as u8;
        self.session.set_volume(v).await
    }

    /// Close the RFCOMM link (does not disconnect the headset from the system).
    pub async fn disconnect(self) -> Result<()> {
        drop(self.session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bdaddr_is_little_endian() {
        assert_eq!(
            parse_bdaddr("A8:E6:E8:6C:6B:B1").unwrap(),
            [0xB1, 0x6B, 0x6C, 0xE8, 0xE6, 0xA8]
        );
        assert!(parse_bdaddr("nope").is_err());
    }

    /// Compile-time check: the app can hold `Device` across tasks.
    #[allow(dead_code)]
    fn assert_send(d: &mut Device) {
        fn req<F: std::future::Future + Send>(_: F) {}
        fn sync<T: Send + Sync>() {}
        sync::<Device>();
        req(Device::connect("A8:E6:E8:6C:6B:B1"));
        req(d.refresh());
        req(d.set_anc(AncMode::Off, 0, false));
        req(d.set_ult(UltMode::Off));
        req(d.set_volume(0));
        req(d.poll(Duration::ZERO));
    }

    #[test]
    fn sockaddr_sizes() {
        assert_eq!(std::mem::size_of::<SockaddrRc>(), 10);
        assert_eq!(std::mem::size_of::<SockaddrL2>(), 14);
    }
}
