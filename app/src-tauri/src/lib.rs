//! Sony Connect desktop app (Tauri v2). Unofficial, not affiliated with Sony.

mod backend;

use backend::{AncMode, Backend, DeviceHandle, DeviceInfo, DeviceState, UltMode};
use serde::Serialize;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tokio::sync::{Mutex, Notify};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const OP_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
}

/// Payload of the "connection" event.
#[derive(Debug, Clone, Serialize)]
struct Connection {
    status: ConnStatus,
    device: Option<DeviceInfo>,
    error: Option<String>,
    /// Seconds until the next reconnect attempt, when disconnected.
    retry_in: Option<u64>,
    backend: &'static str,
}

/// Result of `get_state`.
#[derive(Debug, Clone, Serialize)]
struct Snapshot {
    connection: Connection,
    state: Option<DeviceState>,
}

struct TrayItems {
    tray: TrayIcon<Wry>,
    status: MenuItem<Wry>,
    anc: [(AncMode, CheckMenuItem<Wry>); 3],
    ult: [(UltMode, CheckMenuItem<Wry>); 3],
}

struct Shared {
    backend: Box<dyn Backend>,
    device: Mutex<Option<Box<dyn DeviceHandle>>>,
    connection: StdMutex<Connection>,
    state: StdMutex<Option<DeviceState>>,
    /// Address to (re)connect to; `None` = first connected Sony device.
    target: StdMutex<Option<String>>,
    wake: Notify,
    tray: StdMutex<Option<TrayItems>>,
}

type AppState = Arc<Shared>;

impl Shared {
    fn new(backend: Box<dyn Backend>) -> Self {
        let name = backend.name();
        Self {
            backend,
            device: Mutex::new(None),
            connection: StdMutex::new(Connection {
                status: ConnStatus::Disconnected,
                device: None,
                error: None,
                retry_in: None,
                backend: name,
            }),
            state: StdMutex::new(None),
            target: StdMutex::new(None),
            wake: Notify::new(),
            tray: StdMutex::new(None),
        }
    }

    fn set_connection(
        &self,
        app: &AppHandle,
        status: ConnStatus,
        device: Option<DeviceInfo>,
        error: Option<String>,
        retry_in: Option<u64>,
    ) {
        let c = Connection {
            status,
            device,
            error,
            retry_in,
            backend: self.backend.name(),
        };
        *self.connection.lock().unwrap() = c.clone();
        let _ = app.emit("connection", &c);
        self.sync_tray(app);
    }

    fn set_state(&self, app: &AppHandle, s: DeviceState) {
        *self.state.lock().unwrap() = Some(s.clone());
        let _ = app.emit("state", &s);
        self.sync_tray(app);
    }

    fn update_state(&self, app: &AppHandle, f: impl FnOnce(&mut DeviceState)) {
        let mut s = self.state.lock().unwrap().clone().unwrap_or_default();
        f(&mut s);
        self.set_state(app, s);
    }

    async fn connect_target(&self) -> Result<Box<dyn DeviceHandle>, String> {
        let target = self.target.lock().unwrap().clone();
        let fut = async {
            match target {
                Some(addr) => self.backend.connect(&addr).await,
                None => self.backend.connect_first().await,
            }
        };
        tokio::time::timeout(OP_TIMEOUT, fut)
            .await
            .map_err(|_| "timed out connecting".to_string())?
    }

    /// Refresh from the device. On failure the device is dropped (poller reconnects).
    async fn refresh(&self, app: &AppHandle) -> Result<DeviceState, String> {
        let mut guard = self.device.lock().await;
        let Some(dev) = guard.as_mut() else {
            return Err("not connected".into());
        };
        let res = match tokio::time::timeout(OP_TIMEOUT, dev.refresh()).await {
            Ok(r) => r,
            Err(_) => Err("timed out waiting for headphones".into()),
        };
        match res {
            Ok(s) => {
                drop(guard);
                self.set_state(app, s.clone());
                Ok(s)
            }
            Err(e) => {
                if let Some(dev) = guard.take() {
                    let _ = tokio::time::timeout(Duration::from_secs(3), dev.disconnect()).await;
                }
                drop(guard);
                self.set_connection(
                    app,
                    ConnStatus::Disconnected,
                    None,
                    Some(e.clone()),
                    Some(BACKOFF_MIN.as_secs()),
                );
                Err(e)
            }
        }
    }

    /// Run an operation on the connected device.
    async fn with_device<T, F>(&self, f: F) -> Result<T, String>
    where
        F: for<'a> FnOnce(
            &'a mut Box<dyn DeviceHandle>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>,
        >,
    {
        let mut guard = self.device.lock().await;
        let dev = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
        tokio::time::timeout(OP_TIMEOUT, f(dev))
            .await
            .map_err(|_| "timed out waiting for headphones".to_string())?
    }

    async fn apply_anc(
        &self,
        app: &AppHandle,
        mode: AncMode,
        level: u8,
        focus: bool,
    ) -> Result<(), String> {
        let level = level.min(20);
        self.with_device(|d| d.set_anc(mode, level, focus)).await?;
        self.update_state(app, |s| {
            s.anc = Some(mode);
            s.ambient_level = Some(level);
            s.focus_on_voice = Some(focus);
        });
        Ok(())
    }

    async fn apply_ult(&self, app: &AppHandle, mode: UltMode) -> Result<(), String> {
        self.with_device(|d| d.set_ult(mode)).await?;
        self.update_state(app, |s| s.ult = Some(mode));
        Ok(())
    }

    fn sync_tray(&self, _app: &AppHandle) {
        let tray = self.tray.lock().unwrap();
        let Some(t) = tray.as_ref() else { return };
        let conn = self.connection.lock().unwrap().clone();
        let state = self.state.lock().unwrap().clone().unwrap_or_default();
        let connected = conn.status == ConnStatus::Connected;

        let name = conn
            .device
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "Sony Connect".into());
        let battery = state.battery.as_ref().filter(|_| connected).map(|b| {
            format!(
                "{}%{}",
                b.level,
                if b.charging { " (charging)" } else { "" }
            )
        });
        let line = match conn.status {
            ConnStatus::Connected => match &battery {
                Some(b) => format!("{name} — Battery {b}"),
                None => format!("{name} — Battery unavailable"),
            },
            ConnStatus::Connecting => "Connecting…".into(),
            ConnStatus::Disconnected => "Disconnected".into(),
        };
        let _ = t.status.set_text(&line);
        let _ = t.tray.set_tooltip(Some(&line));
        // appindicator (Linux) has no tooltips; a title label is the closest thing.
        #[cfg(target_os = "linux")]
        let _ = t.tray.set_title(
            battery
                .as_ref()
                .map(|b| b.split(' ').next().unwrap_or(b).to_string()),
        );

        for (mode, item) in &t.anc {
            let _ = item.set_enabled(connected && state.anc.is_some());
            let _ = item.set_checked(state.anc == Some(*mode));
        }
        for (mode, item) in &t.ult {
            let _ = item.set_enabled(connected && state.ult.is_some());
            let _ = item.set_checked(state.ult == Some(*mode));
        }
    }
}

/// Background task: keep a connection, poll every few seconds, reconnect with backoff.
async fn poller(app: AppHandle, sh: AppState) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let connected = sh.device.lock().await.is_some();
        if !connected {
            sh.set_connection(&app, ConnStatus::Connecting, None, None, None);
            match sh.connect_target().await {
                Ok(dev) => {
                    let info = dev.info().clone();
                    let mut slot = sh.device.lock().await;
                    if slot.is_none() {
                        *slot = Some(dev);
                    } else {
                        // A `connect` command won the race; drop ours.
                        let _ = dev.disconnect().await;
                    }
                    drop(slot);
                    backoff = BACKOFF_MIN;
                    sh.set_connection(&app, ConnStatus::Connected, Some(info), None, None);
                    continue; // refresh immediately
                }
                Err(e) => {
                    sh.set_connection(
                        &app,
                        ConnStatus::Disconnected,
                        None,
                        Some(e),
                        Some(backoff.as_secs()),
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = sh.wake.notified() => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        } else {
            let _ = sh.refresh(&app).await;
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = sh.wake.notified() => {}
            }
        }
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
async fn list_devices(sh: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    sh.backend.discover().await
}

#[tauri::command]
async fn connect(
    app: AppHandle,
    sh: State<'_, AppState>,
    address: Option<String>,
) -> Result<DeviceInfo, String> {
    *sh.target.lock().unwrap() = address;
    sh.set_connection(&app, ConnStatus::Connecting, None, None, None);
    let mut slot = sh.device.lock().await;
    if let Some(old) = slot.take() {
        let _ = tokio::time::timeout(Duration::from_secs(3), old.disconnect()).await;
    }
    match sh.connect_target().await {
        Ok(dev) => {
            let info = dev.info().clone();
            *slot = Some(dev);
            drop(slot);
            sh.set_connection(&app, ConnStatus::Connected, Some(info.clone()), None, None);
            sh.wake.notify_one(); // poller refreshes right away
            Ok(info)
        }
        Err(e) => {
            drop(slot);
            sh.set_connection(
                &app,
                ConnStatus::Disconnected,
                None,
                Some(e.clone()),
                Some(BACKOFF_MIN.as_secs()),
            );
            sh.wake.notify_one();
            Err(e)
        }
    }
}

#[tauri::command]
fn get_state(sh: State<'_, AppState>) -> Snapshot {
    Snapshot {
        connection: sh.connection.lock().unwrap().clone(),
        state: sh.state.lock().unwrap().clone(),
    }
}

#[tauri::command]
async fn set_anc(
    app: AppHandle,
    sh: State<'_, AppState>,
    mode: AncMode,
    ambient_level: Option<u8>,
    focus_on_voice: Option<bool>,
) -> Result<(), String> {
    let (cur_level, cur_focus) = {
        let s = sh.state.lock().unwrap();
        let s = s.as_ref();
        (
            s.and_then(|s| s.ambient_level).unwrap_or(10),
            s.and_then(|s| s.focus_on_voice).unwrap_or(false),
        )
    };
    sh.apply_anc(
        &app,
        mode,
        ambient_level.unwrap_or(cur_level),
        focus_on_voice.unwrap_or(cur_focus),
    )
    .await
}

#[tauri::command]
async fn set_ult(app: AppHandle, sh: State<'_, AppState>, mode: UltMode) -> Result<(), String> {
    sh.apply_ult(&app, mode).await
}

#[tauri::command]
async fn set_volume(app: AppHandle, sh: State<'_, AppState>, volume: u8) -> Result<(), String> {
    let volume = volume.min(100);
    sh.with_device(|d| d.set_volume(volume)).await?;
    sh.update_state(&app, |s| s.volume = Some(volume));
    Ok(())
}

// ---------------------------------------------------------------- tray / window

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// True when a StatusNotifier host (the thing that draws tray icons) is on the
/// session bus.
async fn tray_host_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(conn) = zbus::Connection::session().await else {
            return false;
        };
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
            return false;
        };
        let Ok(name) = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher") else {
            return false;
        };
        dbus.name_has_owner(name).await.unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<TrayItems> {
    let status = MenuItem::with_id(app, "status", "Connecting…", false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let anc_nc = CheckMenuItem::with_id(
        app,
        "anc_nc",
        "Noise Cancelling",
        false,
        false,
        None::<&str>,
    )?;
    let anc_amb =
        CheckMenuItem::with_id(app, "anc_amb", "Ambient Sound", false, false, None::<&str>)?;
    let anc_off = CheckMenuItem::with_id(app, "anc_off", "Off", false, false, None::<&str>)?;
    let ult_off = CheckMenuItem::with_id(app, "ult_off", "ULT Off", false, false, None::<&str>)?;
    let ult_1 = CheckMenuItem::with_id(app, "ult_1", "ULT 1", false, false, None::<&str>)?;
    let ult_2 = CheckMenuItem::with_id(app, "ult_2", "ULT 2", false, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &show,
            &sep()?,
            &anc_nc,
            &anc_amb,
            &anc_off,
            &sep()?,
            &ult_off,
            &ult_1,
            &ult_2,
            &sep()?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Sony Connect")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let app = app.clone();
            match id.as_str() {
                "show" => show_main(&app),
                "quit" => app.exit(0),
                _ => {
                    tauri::async_runtime::spawn(async move {
                        let sh = app.state::<AppState>().inner().clone();
                        let res = match id.as_str() {
                            "anc_nc" | "anc_amb" | "anc_off" => {
                                let mode = match id.as_str() {
                                    "anc_nc" => AncMode::NoiseCancelling,
                                    "anc_amb" => AncMode::Ambient,
                                    _ => AncMode::Off,
                                };
                                let (lvl, focus) = {
                                    let s = sh.state.lock().unwrap();
                                    let s = s.as_ref();
                                    (
                                        s.and_then(|s| s.ambient_level).unwrap_or(10),
                                        s.and_then(|s| s.focus_on_voice).unwrap_or(false),
                                    )
                                };
                                sh.apply_anc(&app, mode, lvl, focus).await
                            }
                            "ult_off" => sh.apply_ult(&app, UltMode::Off).await,
                            "ult_1" => sh.apply_ult(&app, UltMode::Ult1).await,
                            "ult_2" => sh.apply_ult(&app, UltMode::Ult2).await,
                            _ => Ok(()),
                        };
                        if let Err(e) = res {
                            log::warn!("tray action {id} failed: {e}");
                        }
                        // Re-sync checkmarks (the toolkit toggles them on click).
                        sh.sync_tray(&app);
                    });
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    // Monochrome glyph: panel icons should be symbolic, not the full-colour app icon.
    match tauri::image::Image::from_bytes(include_bytes!("../tray/tray.png")) {
        Ok(icon) => builder = builder.icon(icon),
        Err(_) => {
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
        }
    }
    let tray = builder.build(app)?;

    Ok(TrayItems {
        tray,
        status,
        anc: [
            (AncMode::NoiseCancelling, anc_nc),
            (AncMode::Ambient, anc_amb),
            (AncMode::Off, anc_off),
        ],
        ult: [
            (UltMode::Off, ult_off),
            (UltMode::Ult1, ult_1),
            (UltMode::Ult2, ult_2),
        ],
    })
}

pub fn run() {
    let shared: AppState = Arc::new(Shared::new(backend::default_backend()));

    tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            list_devices,
            connect,
            get_state,
            set_anc,
            set_ult,
            set_volume
        ])
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Closing hides to the tray, but only if a tray host is actually running:
                // GNOME loads libappindicator fine yet shows nothing without the
                // AppIndicator extension, which would leave the app hidden for good.
                api.prevent_close();
                let has_tray = window.state::<AppState>().tray.lock().unwrap().is_some();
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    if has_tray && tray_host_running().await {
                        let _ = window.hide();
                    } else {
                        window.app_handle().exit(0);
                    }
                });
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            match build_tray(&handle) {
                Ok(items) => *shared.tray.lock().unwrap() = Some(items),
                Err(e) => log::warn!("tray icon unavailable: {e}"),
            }
            shared.sync_tray(&handle);
            tauri::async_runtime::spawn(poller(handle, shared.clone()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Sony Connect");
}
