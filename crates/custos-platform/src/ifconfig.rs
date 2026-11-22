//! BSD-family `ifconfig -a` parser, shared by the macOS and FreeBSD
//! adapters. The two platforms print the same block format (unindented
//! `<device>: flags=...` header, indented `inet`/`status:` continuation
//! lines), so one pure parser serves both. The module is a pure string
//! parser: it compiles and unit-tests on every platform with fixtures —
//! the live adapters live in the cfg-gated `macos`/`bsd` modules.

use crate::controller::InterfaceStatus;

/// True when an `ifconfig` block header carries the `UP` flag (admin up).
fn flags_have_up(header: &str) -> bool {
    // header like "flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST>"
    let Some(open) = header.find('<') else {
        return false;
    };
    let Some(close) = header[open..].find('>') else {
        return false;
    };
    header[open + 1..open + close].split(',').any(|f| f == "UP")
}

/// Parses `ifconfig -a` output into interface statuses. Pure so fixture
/// strings test it anywhere. An interface is `up` when its flags carry UP
/// AND (when a `status:` line exists) the link is active — a radio with no
/// carrier reports `status: inactive` (macOS) or `status: no carrier`
/// (FreeBSD) and must not read as up. The `%zone` suffix on IPv6
/// link-local addresses is stripped for clean reporting.
pub fn parse_ifconfig(output: &str) -> Vec<InterfaceStatus> {
    let mut interfaces: Vec<InterfaceStatus> = Vec::new();
    let mut name: Option<String> = None;
    let mut flags_up = false;
    let mut status_active = true;
    let mut has_status = false;
    let mut addresses: Vec<String> = Vec::new();

    let flush = |interfaces: &mut Vec<InterfaceStatus>,
                 name: &mut Option<String>,
                 flags_up: &mut bool,
                 status_active: &mut bool,
                 has_status: &mut bool,
                 addresses: &mut Vec<String>| {
        if let Some(n) = name.take() {
            let up = *flags_up && (!*has_status || *status_active);
            interfaces.push(InterfaceStatus {
                name: n,
                up,
                addresses: std::mem::take(addresses),
            });
        }
        *flags_up = false;
        *status_active = true;
        *has_status = false;
    };

    for line in output.lines() {
        if line.starts_with(char::is_whitespace) {
            // Continuation line of the current block (indented).
            if name.is_none() {
                continue;
            }
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("status:") {
                has_status = true;
                status_active = rest.trim() == "active";
            } else if let Some(rest) = t.strip_prefix("inet ") {
                if let Some(addr) = rest.split_whitespace().next() {
                    addresses.push(addr.split('%').next().unwrap_or(addr).to_owned());
                }
            } else if let Some(rest) = t.strip_prefix("inet6 ") {
                if let Some(addr) = rest.split_whitespace().next() {
                    addresses.push(addr.split('%').next().unwrap_or(addr).to_owned());
                }
            }
        } else if let Some(colon) = line.find(':') {
            // New block header: "<device>: flags=..."
            flush(
                &mut interfaces,
                &mut name,
                &mut flags_up,
                &mut status_active,
                &mut has_status,
                &mut addresses,
            );
            name = Some(line[..colon].trim().to_owned());
            flags_up = flags_have_up(&line[colon + 1..]);
        }
    }
    flush(
        &mut interfaces,
        &mut name,
        &mut flags_up,
        &mut status_active,
        &mut has_status,
        &mut addresses,
    );
    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_up_interface_with_addresses() {
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	ether b8:09:da:8a:55:b2
	inet6 fe80::ba09:daff:fe8a:55b2%en0 prefixlen 64 secured scopeid 0x8
	inet 192.168.1.5 netmask 0xffffff00 broadcast 192.168.1.255
	status: active
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        let iface = &list[0];
        assert_eq!(iface.name, "en0");
        assert!(iface.up, "{iface:?}");
        assert!(iface.addresses.contains(&"192.168.1.5".to_string()));
        assert!(iface
            .addresses
            .contains(&"fe80::ba09:daff:fe8a:55b2".to_string()));
    }

    #[test]
    fn inactive_link_is_down_despite_up_flag() {
        // macOS: radio off / no carrier — UP flag set but link not active.
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	status: inactive
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        assert!(!list[0].up, "{list:?}");
    }

    #[test]
    fn down_interface_has_no_up_flag() {
        let output = "\
en1: flags=8822<BROADCAST,SMART,SIMPLEX,MULTICAST> mtu 1500
	status: inactive
";
        let list = parse_ifconfig(output);
        assert_eq!(list[0].name, "en1");
        assert!(!list[0].up, "{list:?}");
    }

    #[test]
    fn loopback_is_listed_and_up() {
        let output = "\
lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
	inet 127.0.0.1 netmask 0xff000000
	status: active
";
        let list = parse_ifconfig(output);
        assert!(list.iter().any(|i| i.name == "lo0" && i.up), "{list:?}");
    }

    #[test]
    fn multiple_blocks_and_empty_output() {
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	status: active
utun0: flags=8081<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 2);
        assert!(list[0].up);
        // utun0 has no status line: the UP flag alone stands.
        assert!(list[1].up);
        assert!(parse_ifconfig("").is_empty());
    }

    #[test]
    fn freebsd_no_carrier_is_down_despite_up_flag() {
        // FreeBSD prints `status: no carrier` (not "inactive") for an
        // Ethernet/WLAN with no carrier — the link must not read as up.
        let output = "\
em0: flags=8863<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
	options=81209<RXCSUM,VLAN_MTU,VLAN_HWTAGGING,VLAN_HWCSUM,WOL_MAGIC>
	ether aa:bb:cc:dd:ee:ff
	inet 192.168.1.10 netmask 0xffffff00 broadcast 192.168.1.255
	media: Ethernet autoselect (1000baseT <full-duplex>)
	status: active
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        assert!(list[0].up, "{list:?}");
        assert!(list[0].addresses.contains(&"192.168.1.10".to_string()));
    }

    #[test]
    fn freebsd_wlan_no_carrier_is_down() {
        let output = "\
wlan0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
	ether 04:33:c2:c4:e4:b0
	media: IEEE 802.11 Wireless Ethernet OFDM/54Mbps mode 11g
	status: no carrier
	ssid \"\" channel 1 (2412 MHz 11g)
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "wlan0");
        assert!(!list[0].up, "{list:?}");
    }

    #[test]
    fn freebsd_lo0_is_up_with_dual_stack() {
        let output = "\
lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> metric 0 mtu 16384
	inet6 ::1 prefixlen 128
	inet6 fe80::1%lo0 prefixlen 64 scopeid 0x3
	inet 127.0.0.1 netmask 0xff000000
	status: active
";
        let list = parse_ifconfig(output);
        assert!(list.iter().any(|i| i.name == "lo0" && i.up), "{list:?}");
        let lo = list.iter().find(|i| i.name == "lo0").unwrap();
        // The %lo0 zone suffix is stripped for clean reporting.
        assert!(lo.addresses.contains(&"fe80::1".to_string()), "{lo:?}");
        assert!(lo.addresses.contains(&"127.0.0.1".to_string()), "{lo:?}");
    }
}
