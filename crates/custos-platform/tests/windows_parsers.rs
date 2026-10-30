//! Windows adapter suite (todo.md 210: Windows 一套): the command builders
//! and output parsers are exercised on every platform with real-format
//! fixtures; execution stays `cfg(windows)` and is cross-compile-checked
//! (x86_64-pc-windows-gnu) — never guessed on a non-Windows host.

use custos_platform::windows::{
    ipconfig_flushdns_argv, netsh_interface_reset_argv, netsh_wlan_connect_argv,
    parse_netsh_interfaces, parse_netsh_wlan_state, parse_sc_query, parse_tasklist_csv, sc_argv,
    tasklist_argv, tasklist_has_pid, WindowsProcessController,
};
use custos_platform::{PlatformError, ProcessController};

const TASKLIST_CSV: &str = concat!(
    "\"myapp.exe\",\"4816\",\"Console\",\"1\",\"12,288 K\"\n",
    "\"svchost.exe\",\"932\",\"Services\",\"0\",\"9,216 K\"\n",
    "INFO: No tasks are running which match the specified criteria.\n",
);

const SC_RUNNING: &str = concat!(
    "SERVICE_NAME: W32Time\n",
    "        TYPE               : 10  WIN32_OWN_PROCESS\n",
    "        STATE              : 4  RUNNING\n",
    "                                (STOPPABLE, PAUSABLE, ACCEPTS_SHUTDOWN)\n",
);

const SC_STOPPED: &str = concat!(
    "SERVICE_NAME: wuauserv\n",
    "        STATE              : 1  STOPPED\n",
);

const SC_START_PENDING: &str = "        STATE              : 7  PAUSE_PENDING\n";

const NETSH_INTERFACES: &str = concat!(
    "\n",
    "Admin State    State          Type             Interface Name\n",
    "-------------------------------------------------------------------------\n",
    "Enabled        Connected      Dedicated        Wi-Fi\n",
    "Enabled        Disconnected   Dedicated        Ethernet 2\n",
    "Disabled       Disconnected   Loopback         Loopback Pseudo-Interface 1\n",
);

const NETSH_WLAN: &str = concat!(
    "There is 1 interface on the system:\n",
    "\n",
    "    Name                   : Wi-Fi\n",
    "    Description            : Intel(R) Wi-Fi 6 AX201\n",
    "    State                  : connected\n",
    "    SSID                   : HomeNet\n",
    "    BSSID                  : aa:bb:cc:dd:ee:ff\n",
);

#[test]
fn tasklist_csv_parses_pids_and_skips_info_lines() {
    let processes = parse_tasklist_csv(TASKLIST_CSV);
    assert_eq!(processes.len(), 2);
    assert_eq!(processes[0].pid, 4816);
    assert_eq!(processes[0].name, "myapp.exe");
    assert!(tasklist_has_pid(TASKLIST_CSV, 4816));
    assert!(!tasklist_has_pid(TASKLIST_CSV, 424242));
}

#[test]
fn tasklist_argv_targets_one_pid() {
    let argv = tasklist_argv(4816);
    assert_eq!(argv[0], "tasklist");
    assert!(argv.contains(&"PID eq 4816".to_owned()));
}

#[test]
fn sc_query_states_normalize() {
    assert!(matches!(
        parse_sc_query(SC_RUNNING),
        custos_platform::ServiceState::Running
    ));
    assert!(matches!(
        parse_sc_query(SC_STOPPED),
        custos_platform::ServiceState::Stopped
    ));
    assert!(matches!(
        parse_sc_query(SC_START_PENDING),
        custos_platform::ServiceState::Transitioning
    ));
    // Unknown vocabulary is carried verbatim, never guessed.
    assert!(matches!(
        parse_sc_query("SERVICE_NAME: x\n        STATE              : 99  WEIRD\n"),
        custos_platform::ServiceState::Unknown(_)
    ));
}

#[test]
fn sc_argv_uses_the_right_verbs() {
    assert_eq!(sc_argv("status", "W32Time"), vec!["sc", "query", "W32Time"]);
    assert_eq!(sc_argv("start", "W32Time"), vec!["sc", "start", "W32Time"]);
    assert_eq!(sc_argv("stop", "W32Time"), vec!["sc", "stop", "W32Time"]);
}

#[test]
fn netsh_interface_list_parses_states() {
    let interfaces = parse_netsh_interfaces(NETSH_INTERFACES);
    assert_eq!(interfaces.len(), 2, "loopback row skipped: {interfaces:?}");
    let wifi = interfaces.iter().find(|i| i.name == "Wi-Fi").unwrap();
    assert!(wifi.up);
    let ethernet = interfaces.iter().find(|i| i.name == "Ethernet 2").unwrap();
    assert!(!ethernet.up);
}

#[test]
fn netsh_wlan_state_extracts_connection_and_ssid() {
    let (connected, ssid) = parse_netsh_wlan_state(NETSH_WLAN);
    assert!(connected);
    assert_eq!(ssid.as_deref(), Some("HomeNet"));
}

#[test]
fn wlan_and_reset_argv_are_explicit() {
    assert_eq!(
        netsh_wlan_connect_argv("HomeNet"),
        vec!["netsh", "wlan", "connect", "name=HomeNet"]
    );
    // Reset is admin=disabled then admin=enabled: explicit down+up, never a
    // blanket reset (todo.md P3 风险控制).
    assert_eq!(
        netsh_interface_reset_argv("Wi-Fi"),
        vec![
            "netsh",
            "interface",
            "set",
            "interface",
            "name=Wi-Fi",
            "admin=disabled",
            "admin=enabled"
        ]
    );
    assert_eq!(ipconfig_flushdns_argv(), vec!["ipconfig", "/flushdns"]);
}

#[cfg(not(windows))]
#[test]
fn windows_process_controller_refuses_on_linux_without_faking() {
    let controller = WindowsProcessController;
    let err = controller
        .find_by_name("anything.exe")
        .expect_err("must refuse on non-Windows");
    assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
    assert!(!controller.is_alive(1), "never fakes liveness");
}
