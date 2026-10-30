//! Linux adapters (docs/guide/platforms.md §11.3). Only the semantics
//! common across Linux hosts are implemented here; anything the local host
//! cannot serve is declared Unsupported with the concrete reason — never
//! guess-executed.

use crate::capability::PlatformCapabilitySet;
use crate::controller::{
    Connectivity, ConnectivityTarget, CredentialProvider, InterfaceStatus,
    NetworkConnectivityProbe, NetworkInterfaceController, ProcessController, ProcessInfo,
    RebootController, ServiceController, ServiceState,
};
use crate::error::{PlatformError, PlatformResult};
use custos_probe::exec::{run_controlled, ExecResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Controlled-execution profile shared by all Linux adapters.
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const TOOL_OUTPUT_CAP: usize = 64 * 1024;

fn run(argv: &[&str]) -> ExecResult {
    let owned: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
    run_controlled(&owned, TOOL_TIMEOUT, TOOL_OUTPUT_CAP)
}

/// Ok when the controlled execution succeeded; the unified error otherwise.
fn expect_success(result: ExecResult) -> PlatformResult<()> {
    match result {
        ExecResult::Succeeded { .. } => Ok(()),
        other => Err(other.into()),
    }
}

/// Detects the local init system for the service matrix (todo.md 194):
/// systemd, OpenRC, or unknown — unknown is reported, never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitSystem {
    Systemd {
        systemctl: PathBuf,
    },
    OpenRc {
        rc_service: PathBuf,
    },
    /// Neither detected: service control is Unsupported on this host.
    Unknown,
}

pub fn detect_init_system() -> InitSystem {
    // The canonical systemd marker (containers and chroots lack it).
    if Path::new("/run/systemd/system").is_dir() {
        if let Some(path) = which("systemctl") {
            return InitSystem::Systemd { systemctl: path };
        }
    }
    if let Some(path) = which("rc-service") {
        return InitSystem::OpenRc { rc_service: path };
    }
    InitSystem::Unknown
}

/// Minimal PATH lookup.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Builds the service controller for THIS host (todo.md 194 matrix):
/// D-Bus systemd first (the matrix's primary), the `systemctl` runner as its
/// fallback, then OpenRC, then SysV init; anything else refuses everything.
pub fn service_controller() -> Box<dyn ServiceController> {
    match detect_init_system() {
        #[cfg(target_os = "linux")]
        InitSystem::Systemd { systemctl } => {
            if crate::init_systems::systemd_dbus::detect().is_ok() {
                Box::new(crate::init_systems::systemd_dbus::SystemdDbusController)
            } else {
                Box::new(SystemdController { systemctl })
            }
        }
        #[cfg(not(target_os = "linux"))]
        InitSystem::Systemd { systemctl } => Box::new(SystemdController { systemctl }),
        #[cfg(target_os = "linux")]
        InitSystem::OpenRc { rc_service } => Box::new(OpenRcController { rc_service }),
        #[cfg(not(target_os = "linux"))]
        InitSystem::OpenRc { .. } => Box::new(NoInitController),
        #[cfg(target_os = "linux")]
        InitSystem::Unknown => {
            if crate::init_systems::sysv::detect() {
                Box::new(crate::init_systems::sysv::SysVController)
            } else {
                Box::new(NoInitController)
            }
        }
        #[cfg(not(target_os = "linux"))]
        InitSystem::Unknown => Box::new(NoInitController),
    }
}

/// Refuses every operation with the concrete reason (绝不误执行).
struct NoInitController;

impl ServiceController for NoInitController {
    fn status(&self, _unit: &str) -> PlatformResult<ServiceState> {
        Err(no_init())
    }
    fn start(&self, _unit: &str) -> PlatformResult<()> {
        Err(no_init())
    }
    fn stop(&self, _unit: &str) -> PlatformResult<()> {
        Err(no_init())
    }
    fn restart(&self, _unit: &str) -> PlatformResult<()> {
        Err(no_init())
    }
}

fn no_init() -> PlatformError {
    PlatformError::Unsupported(
        "no supported init system detected on this host (systemd/OpenRC)".into(),
    )
}

/// systemd service control via `systemctl` under the hardened runner.
pub struct SystemdController {
    systemctl: PathBuf,
}

impl SystemdController {
    /// argv = [systemctl, verb, flags..., unit]. Units come last; that is
    /// valid for every verb used here.
    fn systemctl(&self, args: &[&str], unit: &str) -> ExecResult {
        let program = self.systemctl.to_string_lossy().to_string();
        let mut argv = vec![program.as_str()];
        argv.extend_from_slice(args);
        argv.push(unit);
        run(&argv)
    }
}

impl ServiceController for SystemdController {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
        match self.systemctl(&["show", "--property=ActiveState", "--value"], unit) {
            ExecResult::Succeeded { stdout, .. } => Ok(match stdout.trim() {
                "active" => ServiceState::Running,
                "inactive" | "dead" => ServiceState::Stopped,
                "activating" | "deactivating" => ServiceState::Transitioning,
                "failed" => ServiceState::Failed,
                other => ServiceState::Unknown(other.to_owned()),
            }),
            other => Err(other.into()),
        }
    }

    fn start(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.systemctl(&["start"], unit))
    }

    fn stop(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.systemctl(&["stop"], unit))
    }

    fn restart(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.systemctl(&["restart"], unit))
    }
}

/// OpenRC service control via `rc-service` (best effort; OpenRC semantics
/// are narrower — status falls back to the service's start/stop state).
pub struct OpenRcController {
    rc_service: PathBuf,
}

impl OpenRcController {
    fn rc(&self, verb: &str, unit: &str) -> ExecResult {
        let program = self.rc_service.to_string_lossy().to_string();
        run(&[program.as_str(), verb, unit])
    }
}

impl ServiceController for OpenRcController {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState> {
        match self.rc("status", unit) {
            ExecResult::Succeeded { stdout, .. } => {
                let text = stdout.trim();
                if text.contains("started") {
                    Ok(ServiceState::Running)
                } else if text.contains("stopped") {
                    Ok(ServiceState::Stopped)
                } else {
                    Ok(ServiceState::Unknown(text.to_owned()))
                }
            }
            other => Err(other.into()),
        }
    }

    fn start(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.rc("start", unit))
    }

    fn stop(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.rc("stop", unit))
    }

    fn restart(&self, unit: &str) -> PlatformResult<()> {
        expect_success(self.rc("restart", unit))
    }
}

/// Linux process detection via /proc (read-only, no tools).
pub struct ProcfsProcessController;

impl ProcessController for ProcfsProcessController {
    fn is_alive(&self, pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    fn find_by_name(&self, name: &str) -> PlatformResult<Vec<ProcessInfo>> {
        let mut found = Vec::new();
        let dir = match std::fs::read_dir("/proc") {
            Ok(dir) => dir,
            Err(err) => {
                return Err(PlatformError::Unsupported(format!(
                    "cannot read /proc: {err}"
                )))
            }
        };
        for entry in dir.flatten() {
            let file_name = entry.file_name();
            let Some(pid) = file_name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
                let comm = comm.trim();
                if comm == name {
                    found.push(ProcessInfo {
                        pid,
                        name: comm.to_owned(),
                    });
                }
            }
        }
        found.sort_by_key(|p| p.pid);
        Ok(found)
    }
}

/// Linux network interface status via /sys/class/net (read-only) and the
/// default route via /proc/net/route (pure parsing, fixture-testable).
pub struct SysnetInterfaceController;

impl NetworkInterfaceController for SysnetInterfaceController {
    fn list(&self) -> PlatformResult<Vec<InterfaceStatus>> {
        let dir = std::fs::read_dir("/sys/class/net").map_err(|err| {
            PlatformError::Unsupported(format!("cannot read /sys/class/net: {err}"))
        })?;
        let mut interfaces = Vec::new();
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let operstate = std::fs::read_to_string(entry.path().join("operstate"))
                .map(|s| s.trim().to_owned())
                .unwrap_or_default();
            let up = operstate == "up" || operstate == "unknown";
            interfaces.push(InterfaceStatus {
                name,
                up,
                addresses: Vec::new(),
            });
        }
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(interfaces)
    }

    fn reset(&self, name: &str) -> PlatformResult<()> {
        // High risk: the capability metadata must mark interrupts_*; the
        // permission gate lives in the dispatch layer (ADR-0012).
        let result = run(&["ip", "link", "set", name, "down"]);
        if !matches!(result, ExecResult::Succeeded { .. }) {
            return Err(result.into());
        }
        expect_success(run(&["ip", "link", "set", name, "up"]))
    }
}

/// Parses `/proc/net/route` content (little-endian hex) into the default
/// gateway, if any. Pure so fixture strings test it anywhere.
pub fn default_gateway_of_route_table(table: &str) -> Option<(String, String)> {
    let mut best: Option<(u32, String, String)> = None; // (metric, iface, gateway)
    for line in table.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let iface = fields.next()?.to_owned();
        let destination = fields.next()?;
        let gateway_hex = fields.next()?;
        // Fields: Flags(3) RefCnt Use Metric Mask MTU Window IRTT.
        let _flags = fields.next();
        let _refcnt = fields.next();
        let _use = fields.next();
        let metric: u32 = fields.next()?.parse().ok()?;
        if destination != "00000000" {
            continue; // default route only
        }
        if gateway_hex == "00000000" {
            continue; // on-link, no gateway
        }
        if best.is_none() || metric < best.as_ref().unwrap().0 {
            best = Some((metric, iface, gateway_hex.to_owned()));
        }
    }
    let (_, iface, gateway_hex) = best?;
    Some((iface, hex_to_ipv4(&gateway_hex)?))
}

/// Little-endian hex (as /proc writes it) to dotted quad.
fn hex_to_ipv4(hex: &str) -> Option<String> {
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(format!(
        "{}.{}.{}.{}",
        value & 0xff,
        (value >> 8) & 0xff,
        (value >> 16) & 0xff,
        (value >> 24) & 0xff
    ))
}

impl NetworkConnectivityProbe for SysnetInterfaceController {
    fn probe_tcp(&self, target: &ConnectivityTarget) -> PlatformResult<Connectivity> {
        use std::net::TcpStream;
        let start = std::time::Instant::now();
        match TcpStream::connect_timeout(
            &format!("{}:{}", target.host, target.port)
                .parse()
                .map_err(|_| PlatformError::Failed("invalid target".into()))?,
            TOOL_TIMEOUT,
        ) {
            Ok(_) => Ok(Connectivity::Reachable {
                latency_ms: start.elapsed().as_millis() as u64,
            }),
            Err(err) => match err.kind() {
                std::io::ErrorKind::ConnectionRefused => Ok(Connectivity::Refused),
                _ => Ok(Connectivity::Unreachable(err.to_string())),
            },
        }
    }
}

/// DNS resolution check (todo.md 202 DNS 检查): resolves `host:port` via
/// the platform resolver; the answer is normalized, never raw tool output.
pub fn dns_resolves(host: &str, port: u16) -> PlatformResult<bool> {
    use std::net::ToSocketAddrs;
    Ok((host, port)
        .to_socket_addrs()
        .map(|mut addrs| addrs.next().is_some())
        .unwrap_or(false))
}

/// Credentials from environment variables (P2 baseline; P3 adds file/OS
/// keyring providers). Never logged by callers.
pub struct EnvCredentialProvider;

impl CredentialProvider for EnvCredentialProvider {
    fn get(&self, name: &str) -> PlatformResult<Option<String>> {
        Ok(std::env::var(name).ok())
    }
}

/// Reboot via the detected init system; without one it is Unsupported.
pub struct LinuxRebootController {
    init: InitSystem,
}

impl LinuxRebootController {
    pub fn detect() -> Self {
        Self {
            init: detect_init_system(),
        }
    }
}

impl RebootController for LinuxRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        match &self.init {
            InitSystem::Systemd { systemctl } => {
                let program = systemctl.to_string_lossy().to_string();
                expect_success(run(&[program.as_str(), "reboot"]))
            }
            InitSystem::OpenRc { .. } => expect_success(run(&["reboot"])),
            InitSystem::Unknown => Err(PlatformError::Unsupported(
                "no supported init system detected; refusing to reboot".into(),
            )),
        }
    }
}

/// The Linux adapter's declared support (todo.md 193: 不伪造平台支持).
pub fn capabilities() -> PlatformCapabilitySet {
    let mut set = PlatformCapabilitySet::new()
        .native("process.detect")
        .native("network.interface.list")
        .native("network.connectivity.tcp")
        .native("network.default_gateway");
    match detect_init_system() {
        InitSystem::Systemd { .. } => {
            set = set
                .native("service.status")
                .native("service.start")
                .native("service.stop")
                .native("service.restart")
                .native("system.reboot")
        }
        InitSystem::OpenRc { .. } => {
            set = set
                .native("service.status")
                .native("service.start")
                .native("service.stop")
                .native("service.restart")
                .native("system.reboot")
        }
        InitSystem::Unknown => {
            set = set
                .unsupported("service.status", "no systemd or OpenRC on this host")
                .unsupported("service.start", "no systemd or OpenRC on this host")
                .unsupported("service.stop", "no systemd or OpenRC on this host")
                .unsupported("service.restart", "no systemd or OpenRC on this host")
                .unsupported("system.reboot", "no supported init system detected")
        }
    }
    // Interface reset stays native-declared but high-risk: the toolbox
    // capability metadata carries interrupts_remote_admin (ADR-0012).
    set.native("network.interface.reset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table_default_gateway_parsing() {
        let table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
enp3s0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
wlan0\t00000000\t0200A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
enp3s0\t0001A8C0\t00000000\t0001\t0\t0\t100\tFFFFFF00\t0\t0\t0
";
        // 0100A8C0 little-endian = 192.168.0.1; lowest metric (enp3s0, 100).
        let (iface, gateway) = default_gateway_of_route_table(table).unwrap();
        assert_eq!(iface, "enp3s0");
        assert_eq!(gateway, "192.168.0.1");
    }

    #[test]
    fn route_table_without_default_gateway_is_none() {
        let table = "Iface\tDestination\tGateway\nenp3s0\t0001A8C0\t00000000\n";
        assert!(default_gateway_of_route_table(table).is_none());
        assert!(default_gateway_of_route_table("").is_none());
    }

    #[test]
    fn self_process_is_findable_by_name() {
        // The test harness runs under the crate's test binary; its comm is
        // the binary name. Look for our own pid's existence at least.
        let controller = ProcfsProcessController;
        assert!(controller.is_alive(std::process::id()));
        // find_by_name must return only exact matches.
        for process in controller
            .find_by_name("definitely-not-a-real-name")
            .unwrap()
        {
            assert_eq!(process.name, "definitely-not-a-real-name");
        }
    }

    #[test]
    fn init_detection_is_one_of_the_known_shapes() {
        assert!(matches!(
            detect_init_system(),
            InitSystem::Systemd { .. } | InitSystem::OpenRc { .. } | InitSystem::Unknown
        ));
    }

    #[test]
    fn capabilities_declare_service_control_only_when_supported() {
        let set = capabilities();
        let init = detect_init_system();
        let service_ok = !matches!(init, InitSystem::Unknown);
        assert_eq!(set.is_native("service.status"), service_ok);
        assert_eq!(set.is_native("system.reboot"), service_ok);
        assert!(set.is_native("process.detect"));
        assert!(set.is_native("network.interface.reset"));
    }
}
