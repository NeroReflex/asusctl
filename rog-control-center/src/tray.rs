//! A seld-contained tray icon with menus. The control of app<->tray is done via
//! commands over an MPSC channel.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::sleep;
use std::time::Duration;

use betrayer::{Icon, Menu, MenuItem, TrayEvent, TrayIcon, TrayIconBuilder};
use log::{debug, error, info, warn};
use rog_platform::platform::Properties;
use supergfxctl::pci_device::{Device, GfxMode, GfxPower};
use supergfxctl::zbus_proxy::DaemonProxyBlocking as GfxProxy;
use versions::Versioning;

use crate::config::Config;
use crate::{get_ipc_file, QUIT_APP, SHOW_GUI};

const TRAY_LABEL: &str = "ROG Control Center";
const TRAY_ICON_PATH: &str = "/usr/share/icons/hicolor/512x512/apps/";

struct Icons {
    rog_blue: Icon,
    rog_red: Icon,
    rog_green: Icon,
    rog_white: Icon,
    gpu_integrated: Icon,
}

static ICONS: OnceLock<Icons> = OnceLock::new();

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum TrayAction {
    Open,
    Quit,
}

fn open_app() {
    if let Ok(mut ipc) = get_ipc_file().map_err(|e| {
        error!("ROGTray: get_ipc_file: {}", e);
    }) {
        debug!("Tray told app to show self");
        ipc.write_all(&[SHOW_GUI, 0]).ok();
    }
}

fn quit_app() {
    if let Ok(mut ipc) = get_ipc_file().map_err(|e| {
        error!("ROGTray: get_ipc_file: {}", e);
    }) {
        debug!("Tray told app to show self");
        ipc.write_all(&[QUIT_APP, 0]).ok();
    }
}

fn read_icon(file: &Path) -> Icon {
    let mut path = PathBuf::from(TRAY_ICON_PATH);
    path.push(file);
    let mut file = OpenOptions::new().read(true).open(path).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();

    Icon::from_png_bytes(&bytes)
        .unwrap_or(Icon::from_rgba(vec![255u8; 32 * 32 * 4], 32, 32).unwrap())
}

fn build_menu() -> Menu<TrayAction> {
    Menu::new([
        MenuItem::separator(),
        MenuItem::button("Open", TrayAction::Open),
        MenuItem::button("Quit App", TrayAction::Quit),
    ])
}

fn do_action(event: TrayEvent<TrayAction>) {
    if let TrayEvent::Menu(action) = event {
        match action {
            TrayAction::Open => open_app(),
            TrayAction::Quit => {
                quit_app();
                exit(0);
            }
        }
    }
}

fn set_tray_icon_and_tip(
    mode: GfxMode,
    power: GfxPower,
    tray: &mut TrayIcon<TrayAction>,
    supergfx_active: bool,
) {
    if let Some(icons) = ICONS.get() {
        let icon = match power {
            GfxPower::Suspended => icons.rog_blue.clone(),
            GfxPower::Off => {
                if mode == GfxMode::Vfio {
                    icons.rog_red.clone()
                } else {
                    icons.rog_green.clone()
                }
            }
            GfxPower::AsusDisabled => icons.rog_white.clone(),
            GfxPower::AsusMuxDiscreet | GfxPower::Active => icons.rog_red.clone(),
            GfxPower::Unknown => {
                if supergfx_active {
                    icons.gpu_integrated.clone()
                } else {
                    icons.rog_red.clone()
                }
            }
        };
        // *tray = TrayIconBuilder::<TrayAction>::new()
        //     .with_icon(icon)
        //     .with_tooltip(format!("ROG: gpu mode = {mode:?}, gpu power = {power:?}"))
        //     .with_menu(build_menu())
        //     .build(do_action)
        //     .map_err(|e| log::error!("Tray unable to be initialised: {e:?}"))
        //     .unwrap();

        tray.set_icon(Some(icon));
        tray.set_tooltip(format!("ROG: gpu mode = {mode:?}, gpu power = {power:?}"));
    }
}

fn find_dgpu() -> Option<Device> {
    use supergfxctl::pci_device::Device;
    let dev = Device::find().unwrap_or_default();
    for dev in dev {
        if dev.is_dgpu() {
            info!("Found dGPU: {}", dev.pci_id());
            // Plain old thread is perfectly fine since most of this is potentially blocking
            return Some(dev);
        }
    }
    warn!("Did not find a dGPU on this system, dGPU status won't be avilable");
    None
}

/// The tray is controlled somewhat by `Arc<Mutex<SystemState>>`
pub fn init_tray(_supported_properties: Vec<Properties>, config: Arc<Mutex<Config>>) {
    std::thread::spawn(move || {
        let rog_red = read_icon(&PathBuf::from("asus_notif_red.png"));

        if let Ok(mut tray) = TrayIconBuilder::<TrayAction>::new()
            .with_icon(rog_red.clone())
            .with_tooltip(TRAY_LABEL)
            .with_menu(build_menu())
            .build(do_action)
            .map_err(|e| {
                log::error!(
                    "Tray unable to be initialised: {e:?}. Do you have a system tray enabled?"
                )
            })
        {
            info!("Tray started");
            let rog_blue = read_icon(&PathBuf::from("asus_notif_blue.png"));
            let rog_green = read_icon(&PathBuf::from("asus_notif_green.png"));
            let rog_white = read_icon(&PathBuf::from("asus_notif_white.png"));
            let gpu_integrated = read_icon(&PathBuf::from("rog-control-center.png"));
            ICONS.get_or_init(|| Icons {
                rog_blue,
                rog_red: rog_red.clone(),
                rog_green,
                rog_white,
                gpu_integrated,
            });

            let mut has_supergfx = false;
            let conn = zbus::blocking::Connection::system().unwrap();
            if let Ok(gfx_proxy) = GfxProxy::new(&conn) {
                match gfx_proxy.mode() {
                    Ok(_) => {
                        has_supergfx = true;
                        if let Ok(version) = gfx_proxy.version() {
                            if let Some(version) = Versioning::new(&version) {
                                let curr_gfx = Versioning::new("5.2.0").unwrap();
                                warn!("supergfxd version = {version}");
                                if version < curr_gfx {
                                    // Don't allow mode changing if too old a version
                                    warn!("supergfxd found but is too old to use");
                                    has_supergfx = false;
                                }
                            }
                        }
                    }
                    Err(e) => warn!("Couldn't get mode form supergfxd: {e:?}"),
                }

                info!("Started ROGTray");
                let mut last_power = GfxPower::Unknown;
                let dev = find_dgpu();
                loop {
                    sleep(Duration::from_millis(1000));
                    if let Ok(lock) = config.try_lock() {
                        if !lock.enable_tray_icon {
                            return;
                        }
                    }
                    if has_supergfx {
                        if let Ok(mode) = gfx_proxy.mode() {
                            if let Ok(power) = gfx_proxy.power() {
                                if last_power != power {
                                    set_tray_icon_and_tip(mode, power, &mut tray, has_supergfx);
                                    last_power = power;
                                }
                            }
                        }
                    } else if let Some(dev) = dev.as_ref() {
                        if let Ok(power) = dev.get_runtime_status() {
                            if last_power != power {
                                set_tray_icon_and_tip(
                                    GfxMode::Hybrid,
                                    power,
                                    &mut tray,
                                    has_supergfx,
                                );
                                last_power = power;
                            }
                        }
                    }
                }
            }
        }
    });
}
