//! Webhook SSRF protection (docs/guide/notification.md §8.2): blocks
//! delivery to private, loopback and other internal targets per the
//! configured `SsrfPolicy`, and disables redirect following when the policy
//! says so.
//!
//! The check resolves the URL host and tests every resolved address; a host
//! on the allowlist bypasses the block rules. Because the block is a local
//! policy refusal (retrying cannot fix it), violations surface as a
//! terminal error. Note: check-then-connect leaves a small DNS-rebinding
//! window; strict deployments should prefer an allowlisted or literal-IP
//! target (the same caveat applies to most lightweight HTTP clients).

use custos_config::SsrfPolicy;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

/// One SSRF policy refusal, rendered into a terminal channel error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfBlock {
    Loopback,
    Private,
    LinkLocal,
    Cgnat,
    Unspecified,
}

impl SsrfBlock {
    pub fn describe(self) -> &'static str {
        match self {
            SsrfBlock::Loopback => "loopback address",
            SsrfBlock::Private => "private address",
            SsrfBlock::LinkLocal => "link-local address",
            SsrfBlock::Cgnat => "CGNAT (shared) address",
            SsrfBlock::Unspecified => "unspecified address",
        }
    }
}

/// Resolves the URL host and validates every address against the policy.
/// Returns the resolved addresses (for callers that want to pin the
/// connection), or the first block reason.
pub fn validate_target(url: &str, policy: &SsrfPolicy) -> Result<Vec<IpAddr>, SsrfBlock> {
    let parsed = url::Url::parse(url).map_err(|_| SsrfBlock::Unspecified)?;
    let host = parsed.host_str().ok_or(SsrfBlock::Unspecified)?.to_owned();
    // An explicit allowlist entry exempts the host entirely.
    if policy.allowlist.iter().any(|allowed| allowed == &host) {
        return Ok(Vec::new());
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs: Vec<IpAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| SsrfBlock::Unspecified)?
        .map(|sock| sock.ip())
        .collect();
    if addrs.is_empty() {
        return Err(SsrfBlock::Unspecified);
    }
    for addr in &addrs {
        check_ip(*addr, policy)?;
    }
    Ok(addrs)
}

/// Tests one address against the policy's block rules.
pub fn check_ip(ip: IpAddr, policy: &SsrfPolicy) -> Result<(), SsrfBlock> {
    // IPv4-mapped IPv6 (::ffff:a.b.c.d) is really an IPv4 target.
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    };
    if policy.block_loopback && ip.is_loopback() {
        return Err(SsrfBlock::Loopback);
    }
    if !policy.block_private {
        return Ok(());
    }
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() {
                return Err(SsrfBlock::Unspecified);
            }
            if v4.is_private() {
                return Err(SsrfBlock::Private);
            }
            if v4.is_link_local() {
                return Err(SsrfBlock::LinkLocal);
            }
            if is_cgnat(v4) {
                return Err(SsrfBlock::Cgnat);
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                return Err(SsrfBlock::Unspecified);
            }
            if is_unique_local(v6) {
                return Err(SsrfBlock::Private);
            }
            if is_v6_link_local(v6) {
                return Err(SsrfBlock::LinkLocal);
            }
        }
    }
    Ok(())
}

/// 100.64.0.0/10 — carrier-grade NAT (RFC 6598), not a public address.
fn is_cgnat(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 0x40
}

/// fc00::/7 — IPv6 unique local addresses (ULA, RFC 4193).
fn is_unique_local(ip: Ipv6Addr) -> bool {
    ip.segments()[0] & 0xFE00 == 0xFC00
}

/// fe80::/10 — IPv6 link-local.
fn is_v6_link_local(ip: Ipv6Addr) -> bool {
    ip.segments()[0] & 0xFFC0 == 0xFE80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SsrfPolicy {
        SsrfPolicy::default_blocking()
    }

    fn ipv4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_loopback_v4_and_v6() {
        assert_eq!(
            check_ip(ipv4("127.0.0.1"), &policy()),
            Err(SsrfBlock::Loopback)
        );
        assert_eq!(
            check_ip(ipv4("127.255.255.1"), &policy()),
            Err(SsrfBlock::Loopback)
        );
        assert_eq!(
            check_ip("::1".parse().unwrap(), &policy()),
            Err(SsrfBlock::Loopback)
        );
    }

    #[test]
    fn blocks_private_rfc1918() {
        for ip in ["10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1"] {
            assert_eq!(
                check_ip(ipv4(ip), &policy()),
                Err(SsrfBlock::Private),
                "{ip}"
            );
        }
        // 172.32.0.0/12 boundary is public.
        assert_eq!(check_ip(ipv4("172.32.0.1"), &policy()), Ok(()));
    }

    #[test]
    fn blocks_link_local_and_unspecified() {
        assert_eq!(
            check_ip(ipv4("169.254.1.1"), &policy()),
            Err(SsrfBlock::LinkLocal)
        );
        assert_eq!(
            check_ip(ipv4("0.0.0.0"), &policy()),
            Err(SsrfBlock::Unspecified)
        );
        assert_eq!(
            check_ip("::".parse().unwrap(), &policy()),
            Err(SsrfBlock::Unspecified)
        );
    }

    #[test]
    fn blocks_cgnat_shared_address_space() {
        assert_eq!(
            check_ip(ipv4("100.64.0.1"), &policy()),
            Err(SsrfBlock::Cgnat)
        );
        assert_eq!(
            check_ip(ipv4("100.127.255.255"), &policy()),
            Err(SsrfBlock::Cgnat)
        );
        // 100.128.0.0/10 is public.
        assert_eq!(check_ip(ipv4("100.128.0.1"), &policy()), Ok(()));
    }

    #[test]
    fn blocks_ipv6_unique_local_and_link_local() {
        assert_eq!(
            check_ip("fd00::1".parse().unwrap(), &policy()),
            Err(SsrfBlock::Private)
        );
        assert_eq!(
            check_ip("fc00::1".parse().unwrap(), &policy()),
            Err(SsrfBlock::Private)
        );
        assert_eq!(
            check_ip("fe80::1".parse().unwrap(), &policy()),
            Err(SsrfBlock::LinkLocal)
        );
    }

    #[test]
    fn treats_ipv4_mapped_ipv6_as_ipv4() {
        let mapped: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
        assert_eq!(check_ip(mapped, &policy()), Err(SsrfBlock::Private));
        let public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert_eq!(check_ip(public, &policy()), Ok(()));
    }

    #[test]
    fn allows_public_addresses() {
        for ip in ["1.1.1.1", "8.8.8.8", "142.250.72.14"] {
            assert_eq!(check_ip(ipv4(ip), &policy()), Ok(()), "{ip}");
        }
    }

    #[test]
    fn allowlist_bypasses_blocks() {
        let mut p = policy();
        p.allowlist = vec!["internal.example.com".to_owned()];
        // A loopback host named on the allowlist is accepted.
        assert_eq!(
            validate_target("http://internal.example.com/", &p),
            Ok(vec![])
        );
        // Non-allowlisted loopback still blocked.
        assert_eq!(
            validate_target("http://127.0.0.1:8080/", &p),
            Err(SsrfBlock::Loopback)
        );
    }

    #[test]
    fn validate_blocks_loopback_url() {
        let err = validate_target("http://127.0.0.1:8080/hook", &policy());
        assert_eq!(err, Err(SsrfBlock::Loopback));
    }

    #[test]
    fn validate_allows_public_url_and_returns_addrs() {
        let addrs = validate_target("http://example.com:80/hook", &policy()).unwrap();
        assert!(!addrs.is_empty());
    }

    #[test]
    fn disabled_policy_allows_internal() {
        let mut p = policy();
        p.block_loopback = false;
        p.block_private = false;
        assert_eq!(check_ip(ipv4("127.0.0.1"), &p), Ok(()));
        assert_eq!(check_ip(ipv4("10.0.0.1"), &p), Ok(()));
    }
}
