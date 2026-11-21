//! macOS adapter integration (todo.md 241): live `ifconfig`, launchd
//! service matrix and the connectivity probe on THIS macOS host.

// Live-host tests: `ifconfig` and launchd exist only on a macOS host, so
// the file compiles to nothing elsewhere (the linux_adapters.rs gate keeps
// the same discipline).
#![cfg(target_os = "macos")]

use custos_platform::macos::{capabilities, dns_resolves, service_controller};
use custos_platform::{
    Connectivity, ConnectivityTarget, NetworkConnectivityProbe, NetworkInterfaceController,
    ServiceState,
};

#[test]
fn interface_list_contains_loopback() {
    let controller = custos_platform::macos::MacosNetworkController;
    let interfaces = controller.list().expect("ifconfig readable");
    assert!(
        interfaces.iter().any(|i| i.name == "lo0"),
        "loopback must be listed: {interfaces:?}"
    );
}

#[test]
fn connectivity_refusal_is_normalized_not_an_error() {
    let controller = custos_platform::macos::MacosNetworkController;
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
    assert!(matches!(answer, Connectivity::Refused), "{answer:?}");
}

#[test]
fn dns_check_resolves_loopback() {
    let ok = dns_resolves("localhost", 80).unwrap();
    assert!(ok, "localhost must resolve");
}

#[test]
fn launchd_service_matrix_refuses_unknown_units() {
    let controller = service_controller();
    // launchd is the macOS init system: an unknown unit must never read as
    // Running — the honest answers are NotFound / Failed / Unknown. (The
    // runner user may also be denied the privileged system domain; the
    // refusal property holds either way.)
    let outcome = controller.status("definitely-not-a-real-unit-xyz");
    assert!(
        !matches!(outcome, Ok(ServiceState::Running)),
        "a bogus unit must never be reported running: {outcome:?}"
    );
}

#[test]
fn capabilities_declare_launchd_and_network_natively() {
    let set = capabilities();
    assert!(set.is_native("service.status"));
    assert!(set.is_native("network.interface.list"));
    assert!(set.is_native("system.reboot"));
    // Wi-Fi association has no path on macOS (no password/keychain): the
    // operation must refuse rather than guess-execute (§11.1).
    assert!(matches!(
        set.require("wifi.reconnect"),
        Err(custos_platform::PlatformError::Unsupported(_))
    ));
}
