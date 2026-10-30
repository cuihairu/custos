//! Linux adapter integration (todo.md 210: Linux 一套) — live /proc,
//! /sys and the service matrix on THIS host.

use custos_platform::linux::{
    capabilities, detect_init_system, service_controller, InitSystem, ProcfsProcessController,
    SysnetInterfaceController,
};
use custos_platform::{
    ConnectivityTarget, NetworkConnectivityProbe, NetworkInterfaceController, PlatformError,
    ProcessController,
};

#[test]
fn process_controller_finds_its_own_comm() {
    let controller = ProcfsProcessController;
    assert!(controller.is_alive(std::process::id()));
    // Own pid appears in /proc; find_by_name on our own comm finds it.
    let pid = std::process::id();
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap();
    let matches = controller.find_by_name(comm.trim()).unwrap();
    assert!(matches.iter().any(|p| p.pid == pid), "own process listed");
}

#[test]
fn service_matrix_matches_detected_init() {
    let init = detect_init_system();
    let controller = service_controller();
    // Status of a unit that cannot exist: supported inits answer a concrete
    // failure; an unknown init reports Unsupported — never a fake success.
    let outcome = controller.status("definitely-not-a-real-unit-xyz");
    match init {
        InitSystem::Unknown => {
            assert!(
                matches!(outcome, Err(PlatformError::Unsupported(_))),
                "{outcome:?}"
            );
        }
        _ => {
            // Supported init: NotFound / Failed / Busy are all concrete
            // answers; Unsupported would be a lie.
            if let Err(err) = outcome {
                assert!(!matches!(err, PlatformError::Unsupported(_)), "{err}");
            }
        }
    }
}

#[test]
fn interface_list_contains_loopback() {
    let controller = SysnetInterfaceController;
    let interfaces = controller.list().expect("/sys/class/net readable");
    assert!(
        interfaces.iter().any(|i| i.name == "lo"),
        "loopback must be listed: {interfaces:?}"
    );
}

#[test]
fn capabilities_gate_matches_host_reality() {
    let set = capabilities();
    let init = detect_init_system();
    let service_supported = !matches!(init, InitSystem::Unknown);
    assert_eq!(set.is_native("service.restart"), service_supported);
    assert!(set.is_native("process.detect"));
    // Undeclared operations must refuse, never execute (§11.1).
    assert!(matches!(
        set.require("wifi.reconnect"),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn connectivity_refusal_is_normalized_not_an_error() {
    let controller = SysnetInterfaceController;
    // Bind and drop to get a guaranteed refused port.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let answer = controller
        .probe_tcp(&ConnectivityTarget {
            host: "127.0.0.1".into(),
            port,
        })
        .expect("check completes");
    assert!(
        matches!(answer, custos_platform::Connectivity::Refused),
        "{answer:?}"
    );
}
