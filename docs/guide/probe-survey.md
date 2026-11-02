---
title: Industry Survey of Common Probe Types and custos Gap Analysis
---

# 26. Industry Survey of Common Probe Types and Gap Analysis

## 26.1 Overview

This document surveys common check types built into mainstream monitoring/probing tools, compares them against `custos-probe`'s current built-in probing capabilities, and produces a gap analysis that feeds the planning of "custos built-in zero-plugin base probe surface." Survey scope: Nagios/Monitoring Plugins plugin ecosystem, Prometheus blackbox_exporter, Uptime Kuma, Sensu plugin sets, Centreon plugins/connectors, Zabbix agent collection keys, Windows-side NSClient++/check_nt (services/event logs), and the OpenWrt UBUS probe surface.

**Survey scope and primary sources:**

- [Monitoring Plugins manual page index (full check_* list)](https://www.monitoring-plugins.org/doc/man/index.html)
- [nagios-plugins source directory (plugins/)](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins)
- [Prometheus blackbox_exporter configuration docs (prober types)](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)
- [blackbox_exporter README](https://github.com/prometheus/blackbox_exporter/blob/master/README.md)
- [Uptime Kuma README (monitor type list)](https://github.com/louislam/uptime-kuma)
- [Sensu official plugin page (process/disk/memory/http/docker checks)](https://sensu.io/resources/plugins)
- [Centreon centreon-plugins repository (monitored object list)](https://github.com/centreon/centreon-plugins)
- [Centreon connector overview (700+ connector categories)](https://www.centreon.com/connectors/)
- [Zabbix agent collection key docs](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)
- [NSClient++ service and process monitoring scenarios](https://nsclient.org/docs/scenarios/service-monitoring)
- [NSClient++ CheckEventLog reference](https://nsclient.org/docs/reference/windows/CheckEventLog)
- [Nagios XI knowledge base: Service Checks (Windows service/event-log methods)](https://support.nagios.com/kb/print-775.html)
- [Nagios Windows monitoring solutions page](https://www.nagios.com/solutions/Windows-monitoring)
- [OpenWrt Wiki: ubus micro-bus architecture](https://openwrt.org/docs/techref/ubus)

**Current custos state** (evidence from the repository):

- The factory wires 18 native probe kinds: `process` (pid-file liveness, `crates/custos-probe/src/process.rs:7`, factory branch `crates/custos-probe/src/factory.rs:39`), `exec` (controlled command execution: timeout, output cap, process-tree kill on timeout, `crates/custos-probe/src/exec.rs:69`, factory branch `crates/custos-probe/src/factory.rs:142`), `http-endpoint` (HttpProbe, `http.rs:22`), `network-connectivity` (TcpProbe, `tcp.rs:18`), `dns` (DnsProbe, `dns.rs:51`), `tls-certificate` (TlsCertProbe, `tls_cert.rs:26`), `disk` (DiskProbe, `disk.rs:15`), `memory` (MemProbe, `mem.rs:16`), `cpu` (CpuProbe, `cpu.rs:46`), `log` (LogProbe, `log.rs:33`), `systemd` (SystemdProbe, `systemd.rs:23`), `windows-service` (WindowsServiceProbe, `windows_service.rs:20`), `docker-container` (DockerProbe, `docker.rs:21`), `icmp` (PingProbe, `icmp.rs:26`), `network-interface` (NetifProbe, `netif.rs:21`), `default-gateway` (GatewayProbe, `netif.rs:154`), `proxy` (ProxyProbe, `proxy.rs:122`), `database` (DatabaseProbe, `database.rs:51`) — see the factory dispatch `crates/custos-probe/src/factory.rs:38-148`.
- `custom-command` and `tunnel` still delegate to ExecProbe (`crates/custos-probe/src/factory.rs:142-144`); `service` uses the native systemd / windows-service probes when a `unit` is configured, otherwise falls back to exec (`crates/custos-probe/src/factory.rs:114-141`).

Priority is rated for the self-healing engine scenario: high = covers the most common faults and is the primary signal source for self-healing; medium = common but can be temporarily covered by exec or has a narrow scope; low = long tail / industry-specific.

## 26.2 Process / Service

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| Process liveness (pid file) | Nagios [check_procs](https://www.monitoring-plugins.org/doc/man/index.html), [NSClient++ check_process](https://nsclient.org/docs/scenarios/service-monitoring), [Sensu process-checks](https://sensu.io/resources/plugins), [Zabbix proc.num](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/process.rs:10` (factory `factory.rs:29`) | High (exists) | — |
| Controlled command execution (exec) | [Sensu check execution model](https://docs.sensu.io/sensu-go/6.1/operations/monitor-sensu/monitor-sensu-with-sensu), NRPE/check_by_ssh patterns | ✅ `crates/custos-probe/src/exec.rs:69` (factory `factory.rs:37`) | High (exists) | — |
| systemd unit state (active/substate) | [Zabbix systemd.unit.info](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent), [NSClient++ check_service (Linux systemd)](https://nsclient.org/docs/scenarios/service-monitoring) | ✅ `crates/custos-probe/src/factory.rs:114-123` (native `systemd` when `unit` is set; exec fallback with `command`) | High (exists) | — |
| Windows service state (Started/Stopped, auto-start-not-running alert) | [NSClient++ check_service](https://nsclient.org/docs/scenarios/service-monitoring), [check_nt SERVICESTATE](https://support.nagios.com/kb/print-775.html), [Nagios XI Windows approach](https://www.nagios.com/solutions/Windows-monitoring) | ✅ `crates/custos-probe/src/factory.rs:124-131` (native `windows_service` when `unit` is set; exec fallback with `command`) | High (exists) | — |
| Docker container state (running/restart count) | [Uptime Kuma Docker Containers](https://github.com/louislam/uptime-kuma), [Sensu Docker plugin](https://sensu.io/resources/plugins), [Centreon Docker](https://github.com/centreon/centreon-plugins) | ✅ `crates/custos-probe/src/factory.rs:93-95` (`docker` crate, not exec) | High (exists) | — |
| Process-level resource usage (CPU/memory/handle thresholds) | [Zabbix proc.mem / proc.cpu.util](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent), [check_procs memory threshold](https://www.monitoring-plugins.org/doc/man/index.html) | ✅ `crates/custos-probe/src/process.rs:12` (RSS threshold via `rss_threshold_kb`); CPU% and handle thresholds not yet | Medium | On Linux read `/proc/<pid>/stat{,m}` and convert; add CPU%/handle thresholds as an extension |
| Windows event log (error/critical count, event id filter) | [NSClient++ CheckEventLog](https://nsclient.org/docs/reference/windows/CheckEventLog), [Nagios XI event log wizard](https://www.nagios.com/solutions/Windows-monitoring) | ❌ None | Medium | On Windows use Windows EID/`wevtutil qe` with time window and level filter; normalize to "error count within the window" |

## 26.3 HTTP / TCP / Ports

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| HTTP(S) status code / response time | [check_http](https://www.monitoring-plugins.org/doc/man/index.html), [blackbox_exporter http prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Uptime Kuma HTTP(s)](https://github.com/louislam/uptime-kuma), [Zabbix net.tcp.service[https]](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/factory.rs:53-55` (native `HttpProbe`) | High (exists) | — |
| HTTP keyword / regex matching | [Uptime Kuma HTTP(s) Keyword](https://github.com/louislam/uptime-kuma), [check_http -r/-s](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins), [blackbox fail_if_matches_regexp](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md) | ✅ `crates/custos-probe/src/http.rs:25-26` (`contains` + `fail_if_matches`) | High (exists) | — |
| HTTP JSON assertion (json path / value comparison) | [Uptime Kuma HTTP(s) Json Query](https://github.com/louislam/uptime-kuma) | ✅ `crates/custos-probe/src/http.rs:41,100` (`json_path` + `check_json_assertion`) | Medium (exists) | — |
| TCP port connectivity | [check_tcp](https://www.monitoring-plugins.org/doc/man/index.html), [blackbox tcp prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Uptime Kuma Port](https://github.com/louislam/uptime-kuma), [Zabbix net.tcp.port](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/factory.rs:56-74` (native `TcpProbe`, TLS optional) | High (exists) | — |
| ICMP Ping (packet loss / latency) | [check_ping / check_fping](https://www.monitoring-plugins.org/doc/man/index.html), [blackbox icmp prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Uptime Kuma Ping](https://github.com/louislam/uptime-kuma) | ✅ `crates/custos-probe/src/factory.rs:96-98` (native `PingProbe`) | High (exists) | — |
| Protocol handshake (ssh/smtp/imap/pop/ldap/ftp banner) | [check_ssh / check_smtp / check_imap / check_pop / check_ldap / check_ftp](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix net.tcp.service service list](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ None | Medium | TCP connect then read greeting / send probe and match expected reply; do ssh + smtp first (most used) |
| gRPC health check (grpc.health.v1) | [blackbox grpc prober](https://github.com/prometheus/blackbox_exporter/blob/master/README.md), [Uptime Kuma gRPC](https://github.com/louislam/uptime-kuma) | ❌ None | Medium | tonic + standard health proto; Invoke Health check expects SERVING |
| UDP service probe | [check_udp](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix net.udp.service](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ None | Medium | Send payload + expect response regex + timeout; DNS/NTP can serve as dedicated probes instead |
| WebSocket handshake | [blackbox websocket prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Uptime Kuma Websocket](https://github.com/louislam/uptime-kuma) | ❌ None | Low | tokio-tungstenite handshake completion determines liveness; optionally send/expect a message |
| MQTT connection | Uptime Kuma supports MQTT monitoring ([community mention](https://www.facebook.com/thenewstack/posts/uptime-kuma-can-monitor-several-services-including-ping-https-mysql-tcp-port-smt/1749382069827313/); official README does not enumerate it — unverified) | ❌ None | Low | rumqttc connect + subscribe topic success = Healthy |
| RADIUS auth probe | [check_radius](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Low | Access-Request packet + expect Access-Accept; low usage, leave to exec fallback |

## 26.4 Files

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| File existence / age / size threshold | [check_file_age](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Medium | Metadata check: exists + mtime age cap + size floor; pure std, no external deps |
| Log file keyword monitoring (new-line error match) | [check_log](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix log/log.count/logrt](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/factory.rs:90-92` (native `LogProbe`) | High (exists) | — |

## 26.5 Disk / System Metrics

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| Disk space (percentage / absolute threshold) | [check_disk](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix vfs.fs.size](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent), [Sensu disk-checks](https://sensu.io/resources/plugins), [Nagios Disk Space Checks](https://support.nagios.com/kb/category.php?id=186) | ✅ `crates/custos-probe/src/factory.rs:81-83` (native `DiskProbe`); inode usage not yet | High (exists) | — |
| Inode usage | [Sensu check-disk-usage -K/-W](https://github.com/sensu-plugins/sensu-plugins-disk-checks), [check_disk](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Low | Same function as disk-space probe returning files/ffree; add an inode threshold parameter |
| Memory usage (with available semantics) | [Sensu memory-checks](https://sensu.io/resources/plugins), [Zabbix vm.memory.size](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent), [check_ram (Nagios library)](https://support.nagios.com/kb/category.php?id=186) | ✅ `crates/custos-probe/src/factory.rs:84-86` (native `MemProbe`) | High (exists) | — |
| CPU usage / load | [check_load](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix system.cpu.util / system.cpu.load](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/factory.rs:87-89` (native `CpuProbe`) | High (exists) | — |
| Swap usage | [check_swap](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Medium | Linux `/proc/meminfo` SwapTotal/SwapFree; cheap to implement alongside the memory probe |
| NTP clock offset | [check_ntp_time / check_ntp_peer](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Medium | Simple SNTP client (UDP 123, one round trip computes offset), threshold alert |
| Login users count | [check_users](https://www.monitoring-plugins.org/doc/man/index.html), [Zabbix system.users.num](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ None | Low | Parse `utmp` or run `who | wc -l`; limited scope, defer |
| SMART / hardware sensors | [check_ide_smart / check_sensors](https://www.monitoring-plugins.org/doc/man/index.html), [Sensu check-smart](https://github.com/sensu-plugins/sensu-plugins-disk-checks), [Centreon Hardware connectors](https://www.centreon.com/connectors/) | ❌ None | Low | Delegate to `smartctl -H` and parse OK; hardware sensors via lm-sensors output parsing |
| UPS status | [check_ups](https://www.monitoring-plugins.org/doc/man/index.html), [Centreon UPS-PDU category](https://www.centreon.com/connectors/) | ❌ None | Low | Use the NUT protocol (upsmon/upsrw) to judge OB/LB state |

## 26.6 Database

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| MySQL/MariaDB connectivity + light query | [check_mysql / check_mysql_query](https://www.monitoring-plugins.org/doc/man/index.html), [Uptime Kuma MySQL](https://github.com/louislam/uptime-kuma), [Centreon MySQL/MariaDB connectors](https://www.centreon.com/connectors/) | ✅ `crates/custos-probe/src/database.rs:102,154` (`kind: mysql`) | Medium (exists) | — |
| PostgreSQL connectivity + query | [check_pgsql](https://www.monitoring-plugins.org/doc/man/index.html), [Uptime Kuma PostgreSQL](https://github.com/louislam/uptime-kuma), [Centreon PostgreSQL](https://www.centreon.com/connectors/) | ✅ `crates/custos-probe/src/database.rs:103` (`kind: postgres`) | Medium (exists) | — |
| Redis connectivity | [Centreon Redis](https://github.com/centreon/centreon-plugins), [Uptime Kuma Redis](https://www.facebook.com/thenewstack/posts/uptime-kuma-can-monitor-several-services-including-ping-https-mysql-tcp-port-smt/1749382069827313/) | ✅ `crates/custos-probe/src/database.rs:104` (`kind: redis`) | Medium (exists) | — |
| Other databases (MSSQL/Oracle/Mongo) | [check_mssql / check_oracle / check_dbi](https://www.monitoring-plugins.org/doc/man/index.html), [Centreon Databases category](https://github.com/centreon/centreon-plugins) | ❌ None | Low | Long tail; first cover with ExecProbe (`sqlcmd`/`psql` etc.), native later if needed |

## 26.7 Containers / Virtualization

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| Docker container state | See 26.2 row | ✅ `crates/custos-probe/src/factory.rs:93-95` | High (exists) | — |
| Kubernetes workload (pod/deployment readiness) | No public source found (unverified; Centreon/Zabbix both have K8s solutions but evidence was not collected this time) | ❌ None | Low | kube API GET /apis/apps/v1/namespaces/ns/deployments compares readyReplicas; large dependency surface, do last |

## 26.8 Certificates / DNS

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| TLS certificate expiry / chain validation | [check_http -D (cert expiry)](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins), [blackbox probe_ssl_oui / cert chain validation](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Sensu HTTP plugin SSL expiry](https://sensu.io/resources/plugins) | ✅ `crates/custos-probe/src/factory.rs:78-80` (native `TlsCertProbe`) | High (exists) | — |
| DNS record resolution (A/AAAA/CNAME + answer matching) | [check_dns / check_dig](https://www.monitoring-plugins.org/doc/man/index.html), [blackbox dns prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md), [Uptime Kuma DNS Record](https://github.com/louislam/uptime-kuma), [Zabbix net.dns.record](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/factory.rs:75-77` (native `DnsProbe`) | High (exists) | — |
| DoT/DoH | [blackbox dns_over_tls](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md) (DoT); DoH: no public source found (unverified) | ❌ None | Low | DoT: TLS port 853 reusing the DNS probe; DoH deferred |

## 26.9 Network Devices / Base Network

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| SNMP OID collection / threshold | [check_snmp](https://www.monitoring-plugins.org/doc/man/index.html), [Centreon 700+ SNMP connectors](https://www.centreon.com/connectors/), [Uptime Kuma SNMP](https://github.com/louislam/uptime-kuma) | ❌ None | Medium | snmp crate (v2c/v3), OID sampling + threshold comparison; the de-facto standard for network-device scenarios |
| Interface status/traffic (up/down, errors) | [Zabbix net.if.in / net.if.discovery](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent), [check_ifstatus / check_ifoperstatus](https://www.monitoring-plugins.org/doc/man/index.html) | ✅ `crates/custos-probe/src/factory.rs:99-101` (native `NetifProbe`); gateway `factory.rs:102-104` | Medium (exists) | — |
| OpenWrt/UBUS probe surface (network.interface status, service list, device state) | [OpenWrt ubus: `ubus call network.interface.wan status`, `ubus call service list`](https://openwrt.org/docs/techref/ubus) | ❌ None | Medium | If targeting router/gateway scenarios: JSON-RPC over the ubus socket (/var/run/ubus.sock) to read `network.interface.<name>.status` `up` field and `service.list`; naturally connects with the Wi-Fi/IP/gateway probe chain in docs/guide/probe-diagnosis.md §5.1 |
| DHCP service probe | [check_dhcp](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ None | Low | Send DHCPDISCOVER broadcast, expect OFFER; requires raw socket permissions, long tail |

## 26.10 Others

| Probe type | Who uses it in industry | Current custos | Priority | Implementation notes (not implemented) |
|---|---|---|---|---|
| VPN/tunnel end-to-end verification (not just port open, but content expected through the tunnel) | No single standard implementation ([check_http with proxy mode `-I proxy` is closest](https://www.monitoring-plugins.org/doc/man/index.html)) | ⚠️ `tunnel` type delegates to exec (`factory.rs:142-144`); `proxy` type is native `ProxyProbe` (`factory.rs:105-107`) | Medium | Native: on tunnel resources, run "HTTP/TCP request through the tunnel and assert outbound IP/target reachability"; aligns probe surface with self-healing actions (redial / restart tunnel) |
| Push/active heartbeat (monitored side reports in, timeout = dead) | [Uptime Kuma Push](https://github.com/louislam/uptime-kuma) | ❌ None | Low | Custos is primarily an active-probing model; the heartbeat model adds state storage; defer as needed |
| Game server (Steam/Source) | [Uptime Kuma Steam Game Server](https://github.com/louislam/uptime-kuma) | ❌ None | Low | A2S query protocol, long tail |

## 26.11 Recommended Implementation Order

Basis: a self-healing engine needs **zero-plugin coverage of the most common self-healing trigger signals**; implement the high-frequency-fault + low-cost items first, then the dependency-heavy ones.

1. **HTTP(S) endpoint probe** (status code + keyword + response time) — the most common liveness/quality signal; one request covers three things (26.3 first two rows).
2. **TCP port connectivity** — minimal implementation (connect_timeout); the base primitive for the network diagnosis chain.
3. **DNS resolution** — completes the full network diagnosis chain with TCP/HTTP (matches the evidence chain in probe-diagnosis.md §5.1).
4. **TLS certificate expiry** — reuses TLS capability from 2/3, pure threshold logic, high self-healing value (advance warning).
5. **Disk space / memory / CPU load** (system trio, implement together, shared threshold model).
6. **Log file keyword monitoring** (incremental offset + regex) — "panic/OOM in the log" is a high-value self-healing trigger signal.
7. **systemd unit state** (native implementation for the Linux `service` type, replacing the exec-delegation default).
8. **Docker container state** — containerized deployments are widespread; implement via docker.sock or reuse ExecProbe.
9. **ICMP Ping** — mind the permission-degradation path (TCP-ping fallback when no raw socket).
10. **File existence/age + swap** (cheap additions to the system domain).
11. **Windows service state + event log** (when Windows deployments are needed; custos exec/process already have Windows branches, see tasklist/taskkill paths in process.rs).
12. **MySQL/PostgreSQL/Redis connectivity** (pick per user's actual database stack).
13. **SNMP collection + interface status** (network device scenarios).
14. **OpenWrt/UBUS probe surface** (promote to just after 3 if router/gateway is a target scenario).
15. Everything else (gRPC/WebSocket/MQTT/UPS/SMART/K8s/Push etc.) as long-tail demand; continue to rely on ExecProbe for now.

---

*This document was generated from the custos probe-capability survey; custos code evidence is based on `crates/custos-probe` (process.rs 117 lines / exec.rs 338 lines / factory.rs 90 lines). External tool capabilities are authoritative via the attached links.*