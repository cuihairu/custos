//! Windows adapter suite (todo.md 210: Windows 一套): the command builders
//! and output parsers are exercised on every platform with real-format
//! fixtures; execution stays `cfg(windows)` and is cross-compile-checked
//! (x86_64-pc-windows-gnu) — never guessed on a non-Windows host.

use custos_platform::windows::{
    capabilities, has_wifi_adapter, ipconfig_flushdns_argv, netsh_interface_reset_argv,
    netsh_wlan_connect_argv, netsh_wlan_reports_adapter, parse_default_gateway,
    parse_netsh_interfaces, parse_netsh_wlan_state, parse_sc_query, parse_tasklist_csv,
    route_print_argv, sc_argv, tasklist_argv, tasklist_has_pid, NetshNetworkController,
    WindowsProcessController,
};
use custos_platform::{NetworkInterfaceController, PlatformError, ProcessController};

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

#[cfg(not(windows))]
#[test]
fn wifi_adapter_probe_and_connect_refuse_on_linux_without_faking() {
    // The adapter probe and connect path must not guess on non-Windows:
    // no netsh, so no Wi-Fi adapter and connect is Unsupported.
    assert!(!has_wifi_adapter(), "no netsh on Linux: no Wi-Fi adapter");
    let controller = NetshNetworkController;
    let err = controller
        .connect_wifi("HomeNet")
        .expect_err("must refuse on non-Windows");
    assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
}

#[test]
fn wifi_adapter_probe_requires_an_interface_list_not_just_exit_status() {
    // openclaw audit 2026-11-05: `netsh wlan show interfaces` exits 0 even
    // when the host has NO wireless adapter (it prints a "no wireless
    // interface" notice). The probe must inspect the reported interface
    // list, not trust the exit code.
    assert!(!netsh_wlan_reports_adapter(""), "empty output: no adapter");
    assert!(
        !netsh_wlan_reports_adapter("There is no wireless interface on the system."),
        "en localized no-adapter notice"
    );
    assert!(
        !netsh_wlan_reports_adapter("系统上没有无线接口。"),
        "zh-CN localized no-adapter notice"
    );
    // Non-en/zh-CN no-adapter notices and Name labels must still parse
    // (openclaw audit 2026-11-05: matching only Name/名称 and the two
    // notices left wifi.reconnect silently unavailable on those systems).
    assert!(
        !netsh_wlan_reports_adapter("ワイヤレス インターフェイスがシステム上にありません。"),
        "ja localized no-adapter notice"
    );
    assert!(
        !netsh_wlan_reports_adapter("Aucune interface sans fil sur le système."),
        "fr localized no-adapter notice"
    );
    assert!(
        !netsh_wlan_reports_adapter("Es ist keine drahtlose Schnittstelle vorhanden."),
        "de localized no-adapter notice"
    );
    assert!(
        !netsh_wlan_reports_adapter("무선 인터페이스가 시스템에 없습니다."),
        "ko localized no-adapter notice"
    );
    let with_adapter = "\
There is 1 wireless interface on the system

    Name                   : Wi-Fi
    Description            : Intel(R) Wi-Fi 6 AX201
    GUID                   : 12345678-1234-1234-1234-123456789abc
    State                  : connected
";
    assert!(
        netsh_wlan_reports_adapter(with_adapter),
        "a Name field proves the radio exists"
    );
    // A header-like line mentioning "Name" without an interface block is
    // not an adapter.
    assert!(
        !netsh_wlan_reports_adapter("Name                    :"),
        "empty Name value is not an adapter"
    );
    // Localized Name labels still prove the radio exists.
    for label in ["名前", "Nom", "이름", "Имя", "Nombre", "Nome", "Nazwa", "Naam", "Ad"] {
        let localized = format!("    {label} : Wi-Fi\n");
        assert!(
            netsh_wlan_reports_adapter(&localized),
            "localized Name label must prove the radio: {label}"
        );
    }
}

const ROUTE_PRINT: &str =
    "===========================================================================
Interface List
 10... aa bb cc dd ee ff ......Intel(R) Ethernet
===========================================================================

IPv4 Route Table
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0    192.168.1.1    192.168.1.42     25
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
===========================================================================
";

#[test]
fn route_print_default_gateway_extraction() {
    assert_eq!(
        parse_default_gateway(ROUTE_PRINT).as_deref(),
        Some("192.168.1.1")
    );
    assert_eq!(parse_default_gateway("no routes here"), None);
    assert_eq!(route_print_argv(), vec!["route", "print", "-4"]);
}

#[test]
fn route_print_default_gateway_ignores_persistent_only_default_gateway() {
    let output = "\
Active Routes:
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
永久路由:
      0.0.0.0          0.0.0.0    192.168.1.1  Default
";
    assert_eq!(parse_default_gateway(output), None);

    let active_route = "\
Active Routes:
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
        0.0.0.0          0.0.0.0    192.168.1.1  192.168.1.42  25
Persistent Routes:
      0.0.0.0          0.0.0.0    192.168.9.9  Default
";
    assert_eq!(parse_default_gateway(active_route).as_deref(), Some("192.168.1.1"));
}

#[test]
fn capabilities_do_not_claim_unwired_wifi_reconnect() {
    // §11.1: 不得伪造平台支持. On a host with no Wi-Fi adapter the probe
    // answers false and the gate must refuse with the concrete reason,
    // never a guess. (On a real Windows host with a radio the probe
    // answers true and the declaration is native.)
    let set = capabilities();
    if has_wifi_adapter() {
        assert!(
            set.is_native("wifi.reconnect"),
            "with a Wi-Fi adapter present the declaration must be native"
        );
    } else {
        let err = set
            .require("wifi.reconnect")
            .expect_err("no Wi-Fi adapter: wifi.reconnect must not be claimed native");
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err}");
        let reason = err.to_string();
        assert!(
            reason.contains("no Wi-Fi adapter detected"),
            "reason must name the probe outcome, got: {reason}"
        );
    }
    // The rest of the set keeps its wired operations.
    for operation in [
        "process.detect",
        "service.status",
        "service.start",
        "service.stop",
        "service.restart",
        "network.interface.list",
        "network.interface.reset",
        "network.connectivity.tcp",
        "dns.flush",
        "system.reboot",
    ] {
        assert!(
            set.is_native(operation),
            "`{operation}` must stay native; refusing wifi.reconnect must not drop wired operations"
        );
    }
}
