//! `update_and_notify` is responsible for both notifications *and* updating
//! stored statuses about the system state. This is done through either direct,
//! intoify, zbus notifications or similar methods.
//!
//! This module very much functions like a stand-alone app on its own thread.

use std::fmt::Display;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, error, info, warn};
use notify_rust::{Hint, Notification, Timeout, Urgency};
use rog_dbus::zbus_platform::PlatformProxy;
use rog_platform::platform::GpuMode;
use rog_platform::power::AsusPower;
use serde::{Deserialize, Serialize};
use supergfxctl::actions::UserActionRequired as GfxUserAction;
use supergfxctl::pci_device::{GfxMode, GfxPower};
use supergfxctl::zbus_proxy::DaemonProxy as SuperProxy;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use zbus::export::futures_util::StreamExt;

use crate::config::Config;
use crate::error::Result;

const NOTIF_HEADER: &str = "ROG Control";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct EnabledNotifications {
    pub enabled: bool,
    pub receive_notify_gfx: bool,
    pub receive_notify_gfx_status: bool,
}

impl Default for EnabledNotifications {
    fn default() -> Self {
        Self {
            enabled: true,
            receive_notify_gfx: true,
            receive_notify_gfx_status: true,
        }
    }
}

fn start_dpu_status_mon(config: Arc<Mutex<Config>>) {
    use supergfxctl::pci_device::Device;
    let dev = Device::find().unwrap_or_default();
    let mut found_dgpu = false; // just for logging
    for dev in dev {
        if dev.is_dgpu() {
            info!(
                "Found dGPU: {}, starting status notifications",
                dev.pci_id()
            );
            let enabled_notifications_copy = config.clone();
            // Plain old thread is perfectly fine since most of this is potentially blocking
            std::thread::spawn(move || {
                let mut last_status = GfxPower::Unknown;
                loop {
                    std::thread::sleep(Duration::from_millis(1500));
                    if let Ok(status) = dev.get_runtime_status() {
                        if status != GfxPower::Unknown && status != last_status {
                            if let Ok(config) = enabled_notifications_copy.lock() {
                                if !config.notifications.receive_notify_gfx_status
                                    || !config.notifications.enabled
                                {
                                    continue;
                                }
                            }
                            // Required check because status cycles through
                            // active/unknown/suspended
                            do_gpu_status_notif("dGPU status changed:", &status)
                                .show()
                                .unwrap()
                                .on_close(|_| ());
                            debug!("dGPU status changed: {:?}", &status);
                        }
                        last_status = status;
                    }
                }
            });
            found_dgpu = true;
            break;
        }
    }
    if !found_dgpu {
        warn!("Did not find a dGPU on this system, dGPU status won't be avilable");
    }
}

pub fn start_notifications(
    config: Arc<Mutex<Config>>,
    rt: &Runtime,
) -> Result<Vec<JoinHandle<()>>> {
    // Setup the AC/BAT commands that will run on power status change
    let config_copy = config.clone();
    let blocking = rt.spawn_blocking(move || {
        let power = AsusPower::new()
            .map_err(|e| {
                error!("AsusPower: {e}");
                e
            })
            .unwrap();

        let mut last_state = power.get_online().unwrap_or_default();
        loop {
            if let Ok(p) = power.get_online() {
                let mut ac = String::new();
                let mut bat = String::new();
                if let Ok(config) = config_copy.lock() {
                    ac.clone_from(&config.ac_command);
                    bat.clone_from(&config.bat_command);
                }

                if p == 0 && p != last_state {
                    let prog: Vec<&str> = bat.split_whitespace().collect();
                    if prog.len() > 1 {
                        let mut cmd = Command::new(prog[0]);

                        for arg in prog.iter().skip(1) {
                            cmd.arg(*arg);
                        }
                        cmd.spawn()
                            .map_err(|e| error!("AC command error: {e:?}"))
                            .ok();
                    }
                } else if p != last_state {
                    let prog: Vec<&str> = ac.split_whitespace().collect();
                    if prog.len() > 1 {
                        let mut cmd = Command::new(prog[0]);

                        for arg in prog.iter().skip(1) {
                            cmd.arg(*arg);
                        }
                        cmd.spawn()
                            .map_err(|e| error!("AC command error: {e:?}"))
                            .ok();
                    }
                }
                last_state = p;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    let enabled_notifications_copy = config.clone();
    let no_supergfx = move |e: &zbus::Error| {
        error!("zbus signal: receive_notify_gfx_status: {e}");
        warn!("Attempting to start plain dgpu status monitor");
        start_dpu_status_mon(enabled_notifications_copy.clone());
    };

    // GPU MUX Mode notif
    let enabled_notifications_copy = config.clone();
    tokio::spawn(async move {
        let conn = zbus::Connection::system().await.map_err(|e| {
            error!("zbus signal: receive_notify_gpu_mux_mode: {e}");
            e
        })?;
        let proxy = PlatformProxy::new(&conn).await.map_err(|e| {
            error!("zbus signal: receive_notify_gpu_mux_mode: {e}");
            e
        })?;

        let mut actual_mux_mode = GpuMode::Error;
        if let Ok(mode) = proxy.gpu_mux_mode().await {
            actual_mux_mode = GpuMode::from(mode);
        }

        info!("Started zbus signal thread: receive_notify_gpu_mux_mode");
        while let Some(e) = proxy.receive_gpu_mux_mode_changed().await.next().await {
            if let Ok(config) = enabled_notifications_copy.lock() {
                if !config.notifications.enabled || !config.notifications.receive_notify_gfx {
                    continue;
                }
            }
            if let Ok(out) = e.get().await {
                let mode = GpuMode::from(out);
                if mode == actual_mux_mode {
                    continue;
                }
                do_mux_notification("Reboot required. BIOS GPU MUX mode set to", &mode).ok();
            }
        }
        Ok::<(), zbus::Error>(())
    });

    let enabled_notifications_copy = config.clone();
    // GPU Mode change/action notif
    tokio::spawn(async move {
        let conn = zbus::Connection::system().await.map_err(|e| {
            no_supergfx(&e);
            e
        })?;
        let proxy = SuperProxy::builder(&conn).build().await.map_err(|e| {
            no_supergfx(&e);
            e
        })?;
        let _ = proxy.mode().await.map_err(|e| {
            no_supergfx(&e);
            e
        })?;

        let proxy_copy = proxy.clone();
        let mut p = proxy.receive_notify_action().await?;
        tokio::spawn(async move {
            info!("Started zbus signal thread: receive_notify_action");
            while let Some(e) = p.next().await {
                if let Ok(out) = e.args() {
                    let action = out.action();
                    let mode = convert_gfx_mode(proxy.mode().await.unwrap_or_default());
                    match action {
                        supergfxctl::actions::UserActionRequired::Reboot => {
                            do_mux_notification("Graphics mode change requires reboot", &mode)
                        }
                        _ => do_gfx_action_notif(<&str>::from(action), *action, mode),
                    }
                    .map_err(|e| {
                        error!("zbus signal: do_gfx_action_notif: {e}");
                        e
                    })
                    .ok();
                }
            }
        });

        let mut p = proxy_copy.receive_notify_gfx_status().await?;
        tokio::spawn(async move {
            info!("Started zbus signal thread: receive_notify_gfx_status");
            let mut last_status = GfxPower::Unknown;
            while let Some(e) = p.next().await {
                if let Ok(out) = e.args() {
                    let status = out.status;
                    if status != GfxPower::Unknown && status != last_status {
                        if let Ok(config) = enabled_notifications_copy.lock() {
                            if !config.notifications.receive_notify_gfx_status
                                || !config.notifications.enabled
                            {
                                continue;
                            }
                        }
                        // Required check because status cycles through
                        // active/unknown/suspended
                        do_gpu_status_notif("dGPU status changed:", &status)
                            .show_async()
                            .await
                            .unwrap()
                            .on_close(|_| ());
                    }
                    last_status = status;
                }
            }
        });
        Ok::<(), zbus::Error>(())
    });

    Ok(vec![blocking])
}

fn convert_gfx_mode(gfx: GfxMode) -> GpuMode {
    match gfx {
        GfxMode::Hybrid => GpuMode::Optimus,
        GfxMode::Integrated => GpuMode::Integrated,
        GfxMode::NvidiaNoModeset => GpuMode::Optimus,
        GfxMode::Vfio => GpuMode::Vfio,
        GfxMode::AsusEgpu => GpuMode::Egpu,
        GfxMode::AsusMuxDgpu => GpuMode::Ultimate,
        GfxMode::None => GpuMode::Error,
    }
}

fn base_notification<T>(message: &str, data: &T) -> Notification
where
    T: Display,
{
    let mut notif = Notification::new();
    notif
        .appname(NOTIF_HEADER)
        .summary(&format!("{message} {data}"))
        .timeout(Timeout::Milliseconds(3000))
        .hint(Hint::Category("device".into()));
    notif
}

fn do_gpu_status_notif(message: &str, data: &GfxPower) -> Notification {
    let mut notif = base_notification(message, &<&str>::from(data).to_owned());
    let icon = match data {
        GfxPower::Suspended => "asus_notif_blue",
        GfxPower::Off => "asus_notif_green",
        GfxPower::AsusDisabled => "asus_notif_white",
        GfxPower::AsusMuxDiscreet | GfxPower::Active => "asus_notif_red",
        GfxPower::Unknown => "gpu-integrated",
    };
    notif.icon(icon);
    notif
}

fn do_gfx_action_notif(message: &str, action: GfxUserAction, mode: GpuMode) -> Result<()> {
    if matches!(action, GfxUserAction::Reboot) {
        do_mux_notification("Graphics mode change requires reboot", &mode).ok();
        return Ok(());
    }

    let mut notif = Notification::new();
    notif
        .appname(NOTIF_HEADER)
        .summary(&format!("Changing to {mode}. {message}"))
        //.hint(Hint::Resident(true))
        .hint(Hint::Category("device".into()))
        .urgency(Urgency::Critical)
        .timeout(Timeout::Never)
        .icon("dialog-warning")
        .hint(Hint::Transient(true));

    if matches!(action, GfxUserAction::Logout) {
        notif.action("gfx-mode-session-action", "Logout");
        let handle = notif.show()?;
        if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
            if desktop.to_lowercase() == "gnome" {
                handle.wait_for_action(|id| {
                    if id == "gfx-mode-session-action" {
                        let mut cmd = Command::new("gnome-session-quit");
                        cmd.spawn().ok();
                    } else if id == "__closed" {
                        // TODO: cancel the switching
                    }
                });
            } else if desktop.to_lowercase() == "kde" {
                handle.wait_for_action(|id| {
                    if id == "gfx-mode-session-action" {
                        let mut cmd = Command::new("qdbus");
                        cmd.args(["org.kde.ksmserver", "/KSMServer", "logout", "1", "0", "0"]);
                        cmd.spawn().ok();
                    } else if id == "__closed" {
                        // TODO: cancel the switching
                    }
                });
            } else {
                // todo: handle alternatives
            }
        }
    } else {
        notif.show()?;
    }
    Ok(())
}

/// Actual `GpuMode` unused as data is never correct until switched by reboot
fn do_mux_notification(message: &str, m: &GpuMode) -> Result<()> {
    let mut notif = base_notification(message, &m.to_string());
    notif
        .action("gfx-mode-session-action", "Reboot")
        .urgency(Urgency::Critical)
        .icon("system-reboot-symbolic")
        .hint(Hint::Transient(true));
    let handle = notif.show()?;

    std::thread::spawn(|| {
        if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
            if desktop.to_lowercase() == "gnome" {
                handle.wait_for_action(|id| {
                    if id == "gfx-mode-session-action" {
                        let mut cmd = Command::new("gnome-session-quit");
                        cmd.arg("--reboot");
                        cmd.spawn().ok();
                    } else if id == "__closed" {
                        // TODO: cancel the switching
                    }
                });
            } else if desktop.to_lowercase() == "kde" {
                handle.wait_for_action(|id| {
                    if id == "gfx-mode-session-action" {
                        let mut cmd = Command::new("qdbus");
                        cmd.args(["org.kde.ksmserver", "/KSMServer", "logout", "1", "1", "0"]);
                        cmd.spawn().ok();
                    } else if id == "__closed" {
                        // TODO: cancel the switching
                    }
                });
            }
        }
    });
    Ok(())
}
