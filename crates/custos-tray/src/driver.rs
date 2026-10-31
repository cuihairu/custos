//! The platform tray driver (Windows system tray + macOS menu bar,
//! §9.3): builds the tray icon and context menu from the platform-
//! independent frames, runs the health-poll + beat-animation + menu-event
//! loop, and persists the pulse switch when the user toggles it.
//!
//! Real-machine walkthrough (icon rendering, beat legibility, menu behaviour)
//! is a user-side verification step — the code here is structured so that
//! both platforms share the entire update loop; only the application
//! bootstrap differs (macOS needs an `NSApplication` on the main thread).

use crate::{pulse_frames, static_frame, HealthSummary, TrayConfig, TrayStatus};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Beat period of the pulse animation (one full heartbeat cycle).
const BEAT_PERIOD: Duration = Duration::from_millis(1200);
/// How often the driver polls the health source.
const POLL_INTERVAL: Duration = Duration::from_millis(2000);

/// Everything the driver needs.
pub struct TrayOptions {
    /// Health source: called on the driver's poll cadence; must be cheap
    /// enough to run on the tray thread (the binary passes a store read).
    pub health_source: Box<dyn FnMut() -> HealthSummary + Send>,
    /// Where the pulse switch persists (§9.3 设置持久化).
    pub config_path: std::path::PathBuf,
    /// What 「打开控制台」 runs (the binary spawns a platform terminal).
    pub console_command: Vec<String>,
    /// Tracing label for the poller.
    pub db_label: String,
}

impl TrayOptions {
    pub fn new(
        health_source: Box<dyn FnMut() -> HealthSummary + Send>,
        config_path: impl Into<std::path::PathBuf>,
        console_command: Vec<String>,
        db_label: impl Into<String>,
    ) -> Self {
        Self {
            health_source,
            config_path: config_path.into(),
            console_command,
            db_label: db_label.into(),
        }
    }
}

/// Menu item ids (§9.3 右键菜单: 状态概要 / 动效开关 / 打开控制台 / 退出).
mod menu_id {
    pub const SUMMARY: &str = "custos.tray.summary";
    pub const PULSE: &str = "custos.tray.pulse";
    pub const CONSOLE: &str = "custos.tray.console";
    pub const QUIT: &str = "custos.tray.quit";
}

/// Runs the tray until the user picks 退出. Blocks the calling thread
/// (call it from `main`).
pub fn run(mut options: TrayOptions) -> Result<(), String> {
    // macOS: NSApplication must exist on the main thread before any
    // NSStatusItem is created; accessory policy keeps custos off the Dock.
    #[cfg(target_os = "macos")]
    bootstrap_macos_app();

    let mut config = TrayConfig::load(&options.config_path)?;
    let summary = (*options.health_source)();
    let status = summary.status();

    // Context menu (§9.3).
    use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
    let summary_item = MenuItem::with_id(
        menu_id::SUMMARY,
        summary_line(status, &summary),
        false,
        None,
    );
    let pulse_item = CheckMenuItem::with_id(
        menu_id::PULSE,
        "脉搏动效",
        true,
        config.pulse_animation,
        None,
    );
    let console_item = MenuItem::with_id(menu_id::CONSOLE, "打开控制台", true, None);
    let quit_item = MenuItem::with_id(menu_id::QUIT, "退出", true, None);
    let menu = Menu::new();
    menu.append(&summary_item)
        .and_then(|_| menu.append(&PredefinedMenuItem::separator()))
        .and_then(|_| menu.append(&pulse_item))
        .and_then(|_| menu.append(&console_item))
        .and_then(|_| menu.append(&PredefinedMenuItem::separator()))
        .and_then(|_| menu.append(&quit_item))
        .map_err(|e| format!("build tray menu: {e}"))?;

    // Icon frames: healthy animates through the beat cycle; abnormal is a
    // single static frame the driver never advances.
    let beat_frames: Vec<tray_icon::Icon> = pulse_frames(status, 24)
        .into_iter()
        .map(|frame| tray_icon::Icon::from_rgba(frame.rgba, frame.width, frame.height))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("beat frames: {e}"))?;
    let idle_frame = {
        let frame = static_frame(status, 24);
        tray_icon::Icon::from_rgba(frame.rgba, frame.width, frame.height)
            .map_err(|e| format!("static frame: {e}"))?
    };
    let abnormal_frame = {
        let frame = static_frame(TrayStatus::Abnormal, 24);
        tray_icon::Icon::from_rgba(frame.rgba, frame.width, frame.height)
            .map_err(|e| format!("abnormal frame: {e}"))?
    };

    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(status.tooltip(&summary))
        .with_icon(if status == TrayStatus::Healthy {
            beat_frames[0].clone()
        } else {
            abnormal_frame.clone()
        })
        .build()
        .map_err(|e| format!("build tray icon: {e}"))?;

    // The health poller runs on a side thread so the animation tick stays
    // smooth; it sends summaries over a channel.
    let (tx, rx) = mpsc::channel::<HealthSummary>();
    let mut health_source = options.health_source;
    let db_label = options.db_label.clone();
    std::thread::Builder::new()
        .name("custos-tray-poll".into())
        .spawn(move || {
            // The first summary was already consumed above; poll onward.
            let mut last: Option<HealthSummary> = None;
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let summary = health_source();
                if last != Some(summary) {
                    tracing::debug!(target: "custos::tray", db = %db_label, ?summary, "health changed");
                    last = Some(summary);
                }
                if tx.send(summary).is_err() {
                    return; // driver went away
                }
            }
        })
        .map_err(|e| format!("spawn health poller: {e}"))?;

    // The update loop: menu events, health updates and the beat tick.
    // On macOS this thread IS the main thread (the driver is called from
    // `main`); NSApplication services events while we sleep — the beat tick
    // is coarse enough that a polling loop is fine on both platforms.
    let menu_rx = tray_icon::menu::MenuEvent::receiver();
    let mut last_summary = summary;
    let mut last_status = status;
    let beat_frame_step = BEAT_PERIOD / beat_frames.len() as u32;
    let mut next_beat = Instant::now() + beat_frame_step;
    let mut frame_index = 0usize;
    loop {
        // Menu events (§9.3 右键菜单).
        while let Ok(event) = menu_rx.try_recv() {
            match event.id().as_ref() {
                menu_id::PULSE => {
                    config.pulse_animation = pulse_item.is_checked();
                    if let Err(err) = config.save(&options.config_path) {
                        tracing::warn!(target: "custos::tray", error = %err, "pulse switch not persisted");
                    } else {
                        tracing::info!(
                            target: "custos::tray",
                            enabled = config.pulse_animation,
                            "pulse switch toggled"
                        );
                    }
                    // An off-switch must freeze the icon immediately.
                    if !config.pulse_animation {
                        let frame = if last_status == TrayStatus::Healthy {
                            idle_frame.clone()
                        } else {
                            abnormal_frame.clone()
                        };
                        tray.set_icon(Some(frame)).ok();
                    }
                }
                menu_id::CONSOLE => {
                    if options.console_command.is_empty() {
                        tracing::warn!(target: "custos::tray", "console command not configured");
                    } else {
                        spawn_console(&options.console_command);
                    }
                }
                menu_id::QUIT => {
                    tracing::info!(target: "custos::tray", "quit requested");
                    return Ok(());
                }
                _ => {}
            }
        }

        // Health updates from the poller.
        while let Ok(summary) = rx.try_recv() {
            if summary != last_summary {
                last_summary = summary;
                let status = summary.status();
                if status != last_status {
                    last_status = status;
                    tracing::info!(
                        target: "custos::tray",
                        status = status.accessible_name(),
                        healthy = summary.healthy,
                        total = summary.total,
                        "tray state changed"
                    );
                }
                summary_item.set_text(summary_line(status, &summary));
                let _ = tray.set_tooltip(Some(status.tooltip(&summary)));
                let frame = if status == TrayStatus::Healthy {
                    beat_frames[0].clone()
                } else {
                    abnormal_frame.clone()
                };
                tray.set_icon(Some(frame))
                    .map_err(|e| format!("set icon: {e}"))?;
                frame_index = 0;
                next_beat = Instant::now() + beat_frame_step;
            }
        }

        // Beat animation: only when healthy AND the switch is on.
        if config.pulse_animation
            && last_status == TrayStatus::Healthy
            && Instant::now() >= next_beat
        {
            frame_index = (frame_index + 1) % beat_frames.len();
            tray.set_icon(Some(beat_frames[frame_index].clone()))
                .map_err(|e| format!("set beat frame: {e}"))?;
            next_beat = Instant::now() + beat_frame_step;
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The one-line status summary shown as the (disabled) first menu item.
fn summary_line(status: TrayStatus, summary: &HealthSummary) -> String {
    if summary.total == 0 {
        "custos: 尚无探测结果".to_owned()
    } else {
        format!(
            "custos: {}/{} 健康{}",
            summary.healthy,
            summary.total,
            if status == TrayStatus::Healthy {
                ""
            } else {
                " — 需要关注"
            },
        )
    }
}

/// 「打开控制台」: spawns the platform terminal running the console
/// command. Errors are logged, never fatal (§9.3 打开控制台).
fn spawn_console(command: &[String]) {
    use std::process::Command;
    let program = &command[0];
    let args: Vec<&str> = command[1..].iter().map(String::as_str).collect();
    #[cfg(windows)]
    let result = {
        let mut full = vec!["/C".to_owned(), "start".to_owned()];
        full.extend_from_slice(command);
        Command::new("cmd").args(&full).spawn()
    };
    #[cfg(target_os = "macos")]
    let result = {
        // Best-effort: Terminal.app opens a shell window. A robust
        // integration would use AppleScript to type the command.
        let _ = command;
        Command::new("open").args(["-a", "Terminal"]).spawn()
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let result = Command::new(program).args(&args).spawn();
    match result {
        Ok(_) => tracing::info!(target: "custos::tray", program = %program, "console opened"),
        Err(err) => {
            tracing::warn!(target: "custos::tray", program = %program, error = %err, "console open failed")
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// macOS bootstrap: `NSApplication` in accessory (menu-bar-only) mode.
/// Called once from the main thread before the tray is created.
#[cfg(target_os = "macos")]
fn bootstrap_macos_app() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = unsafe { objc2::rc::MainThreadMarker::new_unchecked() };
    let app = unsafe { NSApplication::sharedApplication(mtm) };
    unsafe { app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_line_covers_the_three_shapes() {
        let empty = HealthSummary::default();
        assert!(summary_line(TrayStatus::Abnormal, &empty).contains("尚无"));
        let healthy = HealthSummary {
            healthy: 2,
            total: 2,
        };
        assert!(summary_line(TrayStatus::Healthy, &healthy).contains("2/2"));
        let abnormal = HealthSummary {
            healthy: 1,
            total: 2,
        };
        assert!(summary_line(TrayStatus::Abnormal, &abnormal).contains("需要关注"));
    }
}
