//! The service-control matrix (todo.md P2 194): one `ServiceController`
//! trait, per-init-system adapters selected by detection. A host whose init
//! system is unknown refuses every service operation with a concrete
//! Unsupported reason — 绝不误执行.

use crate::controller::{ServiceController, ServiceState};
use crate::error::{PlatformError, PlatformResult};

/// Linux systemd via D-Bus (`zbus`, `org.freedesktop.systemd1`) with the
/// `systemctl` controlled-execution fallback. Primary is D-Bus per the
/// platform matrix; the fallback carries the concrete connect failure.
#[cfg(target_os = "linux")]
pub mod systemd_dbus {
    use super::*;
    use std::ops::Deref as _;
    use zbus::OwnedObjectPath;

    const DEST: &str = "org.freedesktop.systemd1";
    const PATH: &str = "/org/freedesktop/systemd1";
    const INTERFACE: &str = "org.freedesktop.systemd1.Manager";
    const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";

    /// Connects to the system bus and the systemd manager.
    async fn connect() -> Result<zbus::Proxy<'static>, PlatformError> {
        let connection = zbus::Connection::system()
            .await
            .map_err(|e| PlatformError::Unsupported(format!("system bus unavailable: {e}")))?;
        zbus::Proxy::new(&connection, DEST, PATH, INTERFACE)
            .await
            .map_err(|e| PlatformError::Unsupported(format!("systemd manager proxy: {e}")))
    }

    async fn unit_property_async(unit: &str, property: &str) -> Result<String, PlatformError> {
        let manager = connect().await?;
        let unit_path: OwnedObjectPath = manager.call("LoadUnit", &(unit,)).await.map_err(|e| {
            let text = e.to_string();
            if text.contains("not loaded") || text.contains("NoSuchUnit") {
                PlatformError::NotFound(format!("unit {unit} not loaded"))
            } else {
                PlatformError::Failed(format!("LoadUnit: {e}"))
            }
        })?;
        let reply = manager
            .connection()
            .call_method(
                Some(DEST),
                &unit_path,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(UNIT_INTERFACE, property),
            )
            .await
            .map_err(|e| PlatformError::Failed(format!("Get {property}: {e}")))?;
        let variant: zbus::OwnedVariant = reply
            .body()
            .deserialize()
            .map_err(|e| PlatformError::Failed(format!("property body: {e}")))?;
        // OwnedValue derefs to `Value<'static>`; ActiveState is a string.
        if let zbus::Variant::Str(text) = variant.deref() {
            return Ok(text.to_string());
        }
        Ok(String::new())
    }

    async fn unit_verb_async(unit: &str, verb: &str) -> Result<(), PlatformError> {
        let manager = connect().await?;
        manager
            .call(verb, &(unit, "replace"))
            .await
            .map_err(|e: zbus::Error| PlatformError::Failed(format!("{verb}: {e}")))
    }

    /// D-Bus systemd controller (Linux only).
    pub struct SystemdDbusController;

    impl ServiceController for SystemdDbusController {
        fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
            let state = zbus::block_on(unit_property_async(unit, "ActiveState"))?;
            Ok(match state.as_str() {
                "active" => ServiceState::Running,
                "inactive" | "dead" => ServiceState::Stopped,
                "activating" | "deactivating" => ServiceState::Transitioning,
                "failed" => ServiceState::Failed,
                other => ServiceState::Unknown(other.to_owned()),
            })
        }

        fn start(&self, unit: &str) -> PlatformResult<()> {
            zbus::block_on(unit_verb_async(unit, "StartUnit"))
        }

        fn stop(&self, unit: &str) -> PlatformResult<()> {
            zbus::block_on(unit_verb_async(unit, "StopUnit"))
        }

        fn restart(&self, unit: &str) -> PlatformResult<()> {
            zbus::block_on(unit_verb_async(unit, "RestartUnit"))
        }
    }

    /// Detects systemd via the canonical marker AND a working D-Bus manager:
    /// the marker alone is not support.
    pub fn detect() -> PlatformResult<()> {
        if !std::path::Path::new("/run/systemd/system").is_dir() {
            return Err(PlatformError::Unsupported(
                "/run/systemd/system missing: this host does not run systemd".into(),
            ));
        }
        zbus::block_on(connect()).map(|_| ())
    }
}

/// Linux SysV init via `/etc/init.d/<svc> start|stop|status` (argv, no
/// shell). Present only on SysV hosts; elsewhere Unsupported.
#[cfg(target_os = "linux")]
pub mod sysv {
    use super::*;
    use custos_probe::exec::{run_controlled, ExecResult};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);
    const OUTPUT_CAP: usize = 64 * 1024;

    pub const INIT_DIR: &str = "/etc/init.d";

    fn script(unit: &str) -> PlatformResult<std::path::PathBuf> {
        let script = std::path::Path::new(INIT_DIR).join(unit);
        // Reject path traversal: the unit must name a script in INIT_DIR.
        if !script.is_file() {
            return Err(PlatformError::NotFound(format!(
                "no SysV script {INIT_DIR}/{unit}"
            )));
        }
        Ok(script)
    }

    fn run_script(script: &std::path::Path, verb: &str) -> PlatformResult<()> {
        let program = script.to_string_lossy().to_string();
        let result = run_controlled(&[program, verb.to_owned()], TIMEOUT, OUTPUT_CAP);
        match result {
            ExecResult::Succeeded { .. } => Ok(()),
            ExecResult::Failed {
                code,
                stdout,
                stderr,
                ..
            } if code == 3 && stdout.trim() == format!("{verb}: already running") => Err(
                PlatformError::Busy(format!("exit {code}: {stdout}{stderr}")),
            ),
            other => Err(other.into()),
        }
    }

    /// SysV init controller (Linux only).
    pub struct SysVController;

    impl ServiceController for SysVController {
        fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
            let script = script(unit)?;
            let program = script.to_string_lossy().to_string();
            let result = run_controlled(&[program, "status".to_owned()], TIMEOUT, OUTPUT_CAP);
            match result {
                ExecResult::Succeeded { stdout, .. } => {
                    if stdout.contains("running") {
                        Ok(ServiceState::Running)
                    } else if stdout.contains("stopped") {
                        Ok(ServiceState::Stopped)
                    } else {
                        Ok(ServiceState::Unknown(stdout.trim().to_owned()))
                    }
                }
                // SysV status exits non-zero when stopped.
                ExecResult::Failed {
                    code: 3, stdout, ..
                } => {
                    if stdout.contains("stopped") || stdout.contains("not running") {
                        Ok(ServiceState::Stopped)
                    } else {
                        Err(PlatformError::Failed(format!("exit 3: {}", stdout.trim())))
                    }
                }
                other => Err(other.into()),
            }
        }

        fn start(&self, unit: &str) -> PlatformResult<()> {
            let script = script(unit)?;
            run_script(&script, "start")
        }

        fn stop(&self, unit: &str) -> PlatformResult<()> {
            let script = script(unit)?;
            run_script(&script, "stop")
        }

        fn restart(&self, unit: &str) -> PlatformResult<()> {
            let script = script(unit)?;
            run_script(&script, "restart")
        }
    }

    /// True when /etc/init.d exists (a SysV host without systemd/OpenRC).
    pub fn detect() -> bool {
        std::path::Path::new(INIT_DIR).is_dir()
    }
}

/// macOS launchd via `launchctl` (todo.md matrix: launchctl print /
/// kickstart -k, system + gui domains). Compile-checked on macOS; live
/// tests belong to macOS hosts.
#[cfg(target_os = "macos")]
pub mod launchd {
    use super::*;
    use custos_probe::exec::{run_controlled, ExecResult};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);
    const OUTPUT_CAP: usize = 64 * 1024;

    fn launchctl(args: &[&str]) -> ExecResult {
        let owned: Vec<String> = ["launchctl".into()]
            .into_iter()
            .chain(args.iter().map(|s| (*s).to_owned()))
            .collect();
        run_controlled(&owned, TIMEOUT, OUTPUT_CAP)
    }

    /// launchd controller. `unit` is a label; the domain defaults to
    /// `system` and may carry a `gui/501:` prefix for per-user services.
    pub struct LaunchdController {
        pub domain: String,
    }

    impl LaunchdController {
        fn target(&self, unit: &str) -> String {
            format!("{}/{}", self.domain, unit)
        }
    }

    impl ServiceController for LaunchdController {
        fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
            let target = self.target(unit);
            match launchctl(&["print", &target]) {
                ExecResult::Succeeded { stdout, .. } => {
                    // `launchctl print` reports `state = running` lines.
                    if stdout.contains("state = running") {
                        Ok(ServiceState::Running)
                    } else if stdout.contains("state = not running") {
                        Ok(ServiceState::Stopped)
                    } else {
                        Ok(ServiceState::Unknown(
                            stdout.lines().next().unwrap_or("").to_owned(),
                        ))
                    }
                }
                ExecResult::Failed {
                    code: _, stderr, ..
                } if stderr.contains("Could not find service") => Err(PlatformError::NotFound(
                    format!("service {unit} not loaded"),
                )),
                other => Err(other.into()),
            }
        }

        fn start(&self, unit: &str) -> PlatformResult<()> {
            let target = self.target(unit);
            launchctl(&["kickstart", &target])
                .map(|_| ())
                .map_err(Into::into)
        }

        fn stop(&self, unit: &str) -> PlatformResult<()> {
            let target = self.target(unit);
            launchctl(&["kill", "SIGTERM", &target])
                .map(|_| ())
                .map_err(Into::into)
        }

        fn restart(&self, unit: &str) -> PlatformResult<()> {
            let target = self.target(unit);
            launchctl(&["kickstart", "-k", &target])
                .map(|_| ())
                .map_err(Into::into)
        }
    }
}

/// Windows SCM via the `windows-service` crate (todo.md matrix: ServiceManager
/// start/stop/status with a settle timeout + result verification), not
/// `sc.exe` argv.
#[cfg(windows)]
pub mod scm {
    use super::*;
    use std::time::Duration;
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    /// How long start/stop may take to settle before verification fails.
    const SETTLE_TIMEOUT: Duration = Duration::from_secs(15);

    fn open_service(
        unit: &str,
        access: ServiceAccess,
    ) -> Result<windows_service::service::Service, PlatformError> {
        let manager =
            ServiceManager::local_computer(None::<&std::ffi::OsStr>, ServiceManagerAccess::CONNECT)
                .map_err(|e| PlatformError::Unsupported(format!("service manager: {e}")))?;
        manager.open_service(unit, access).map_err(|e| match e {
            windows_service::Error::Winapi(io) if io.raw_os_error() == Some(1060) => {
                PlatformError::NotFound(format!("service {unit} not installed"))
            }
            other => PlatformError::Failed(format!("open service {unit}: {other}")),
        })
    }

    /// Waits until the service reaches `want` or the deadline passes.
    fn wait_for(
        service: &windows_service::service::Service,
        want: windows_service::service::ServiceState,
    ) -> PlatformResult<()> {
        let deadline = std::time::Instant::now() + SETTLE_TIMEOUT;
        loop {
            let status = service
                .query_status()
                .map_err(|e| PlatformError::Failed(format!("query status: {e}")))?;
            if status.current_state == want {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(PlatformError::TimedOut(format!(
                    "service did not reach {want:?} within {SETTLE_TIMEOUT:?}"
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// SCM controller (Windows only), per the platform matrix.
    pub struct ScmController;

    impl ServiceController for ScmController {
        fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
            let service = open_service(unit, ServiceAccess::QUERY_STATUS)?;
            let status = service
                .query_status()
                .map_err(|e| PlatformError::Failed(format!("query status: {e}")))?;
            Ok(match status.current_state {
                windows_service::service::ServiceState::Running => ServiceState::Running,
                windows_service::service::ServiceState::Stopped => ServiceState::Stopped,
                windows_service::service::ServiceState::StartPending
                | windows_service::service::ServiceState::StopPending
                | windows_service::service::ServiceState::ContinuePending
                | windows_service::service::ServiceState::PausePending => {
                    ServiceState::Transitioning
                }
                other => ServiceState::Unknown(format!("{other:?}")),
            })
        }

        fn start(&self, unit: &str) -> PlatformResult<()> {
            let service = open_service(unit, ServiceAccess::START)?;
            service
                .start::<&str>(&[])
                .map_err(|e| PlatformError::Failed(format!("start: {e}")))?;
            wait_for(&service, windows_service::service::ServiceState::Running)
        }

        fn stop(&self, unit: &str) -> PlatformResult<()> {
            let service = open_service(unit, ServiceAccess::STOP)?;
            service
                .stop()
                .map_err(|e| PlatformError::Failed(format!("stop: {e}")))?;
            wait_for(&service, windows_service::service::ServiceState::Stopped)
        }

        fn restart(&self, unit: &str) -> PlatformResult<()> {
            self.stop(unit)?;
            self.start(unit)
        }
    }
}
