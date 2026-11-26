//! FreeBSD adapter integration (todo.md 242): live `ifconfig`, rc.d service
//! matrix and the connectivity probe on THIS FreeBSD host.

// Live-host tests: `ifconfig` and rc.d exist only on a FreeBSD host, so
// the file compiles to nothing elsewhere (the macos_adapters.rs gate keeps
// the same discipline).
#![cfg(target_os = "freebsd")]

use custos_platform::bsd::{capabilities, dns_resolves, service_controller};
use custos_platform::{
    Connectivity, ConnectivityTarget, NetworkConnectivityProbe, NetworkInterfaceController,
    ServiceState,
};

#[test]
fn interface_list_contains_loopback() {
    let controller = custos_platform::bsd::BsdNetworkController;
    let interfaces = controller.list().expect("ifconfig readable");
    assert!(
        interfaces.iter().any(|i| i.name == "lo0"),
        "loopback must be listed: {interfaces:?}"
    );
}

#[test]
fn connectivity_refusal_is_normalized_not_an_error() {
    let controller = custos_platform::bsd::BsdNetworkController;
    // Bind and drop to get a refused port. The port can be grabbed by
    // another socket in the tiny window between the drop and the probe
    // (observed flake: the probe reported Reachable on such a collision),
    // so retry until we actually see a refusal.
    let mut answer = None;
    for _ in 0..16 {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let probe = controller
            .probe_tcp(&ConnectivityTarget {
                host: "127.0.0.1".into(),
                port,
            })
            .expect("check completes");
        if !matches!(probe, Connectivity::Reachable { .. }) {
            answer = Some(probe);
            break;
        }
    }
    let answer = answer.expect("probing freshly dropped ports never saw a refusal");
    assert!(matches!(answer, Connectivity::Refused), "{answer:?}");
}

#[test]
fn dns_check_resolves_loopback() {
    let ok = dns_resolves("localhost", 80).unwrap();
    assert!(ok, "localhost must resolve");
}

#[test]
fn rcd_service_matrix_refuses_unknown_units() {
    let controller = service_controller();
    // rc.d is the FreeBSD init system: an unknown unit must never read as
    // Running — the honest answers are NotFound / Failed / Unknown. (The
    // runner user may also be denied the privileged rc.d path; the refusal
    // property holds either way.)
    let outcome = controller.status("definitely-not-a-real-unit-xyz");
    assert!(
        !matches!(outcome, Ok(ServiceState::Running)),
        "a bogus unit must never be reported running: {outcome:?}"
    );
}

#[test]
fn capabilities_declare_rcd_and_network_natively() {
    let set = capabilities();
    assert!(set.is_native("service.status"));
    assert!(set.is_native("network.interface.list"));
    assert!(set.is_native("system.reboot"));
    // Wi-Fi association has no path on FreeBSD (no password): the operation
    // must refuse rather than guess-execute (§11.1).
    assert!(matches!(
        set.require("wifi.reconnect"),
        Err(custos_platform::PlatformError::Unsupported(_))
    ));
}
