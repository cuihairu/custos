---
title: 常见探测类型业界调研与 custos 差距分析
---

# 26. 探测类型业界调研与差距分析

## 26.1 概述

本文调研业界主流监控/探测工具内置的常见检查类型，与 `custos-probe` 当前内置探测能力对比，产出差距分析，作为「custos 内置零插件基础探测面」的规划输入。调研范围：Nagios/Monitoring Plugins 插件生态、Prometheus blackbox_exporter、Uptime Kuma、Sensu 插件集、Centreon 插件/连接器、Zabbix agent 采集键、Windows 侧 NSClient++/check_nt（服务/事件日志）、OpenWrt UBUS 探测面。

**调研范围与主要来源：**

- [Monitoring Plugins 手册页索引（check_* 全清单）](https://www.monitoring-plugins.org/doc/man/index.html)
- [nagios-plugins 源码目录（plugins/）](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins)
- [Prometheus blackbox_exporter 配置文档（prober 类型）](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)
- [blackbox_exporter README](https://github.com/prometheus/blackbox_exporter/blob/master/README.md)
- [Uptime Kuma README（监控类型清单）](https://github.com/louislam/uptime-kuma)
- [Sensu 官方插件页（process/disk/memory/http/docker checks）](https://sensu.io/resources/plugins)
- [Centreon centreon-plugins 仓库（可监控对象清单）](https://github.com/centreon/centreon-plugins)
- [Centreon 连接器总览（700+ 连接器分类）](https://www.centreon.com/connectors/)
- [Zabbix agent 采集键文档](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)
- [NSClient++ 服务与进程监控场景](https://nsclient.org/docs/scenarios/service-monitoring)
- [NSClient++ CheckEventLog 参考](https://nsclient.org/docs/reference/windows/CheckEventLog)
- [Nagios XI 知识库：Service Checks（Windows 服务/事件日志多种查法）](https://support.nagios.com/kb/print-775.html)
- [Nagios Windows 监控方案页](https://www.nagios.com/solutions/Windows-monitoring)
- [OpenWrt Wiki：ubus 微总线架构](https://openwrt.org/docs/techref/ubus)

**custos 现状摘要**（证据来自仓库）：

- 已内置探针仅两种：`ProcessProbe`（pid 文件存活，本地路径 `crates/custos-probe/src/process.rs:10`，工厂分支 `crates/custos-probe/src/factory.rs:29`）与 `ExecProbe`（受控命令执行：超时、输出上限、超时杀进程树，`crates/custos-probe/src/exec.rs:69`、`crates/custos-probe/src/exec.rs:211`，工厂分支 `crates/custos-probe/src/factory.rs:37`）。
- 资源类型 `custom-command`、`network-connectivity`、`http-endpoint`、`proxy`、`tunnel`、`service` 在工厂里**全部委托给 ExecProbe**，由用户在 config 里自备命令（`crates/custos-probe/src/factory.rs:37-42`）——即这些类型**没有原生实现**，开箱即用能力为零插件目标下的主要缺口。

优先级按自愈引擎场景评定：高 = 覆盖最常见故障且是自愈触发的主要信号源；中 = 常见但可暂由 exec 兜底或场景较窄；低 = 长尾/特定行业。

## 26.2 进程 / 服务

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| 进程存活（pid 文件） | Nagios [check_procs](https://www.monitoring-plugins.org/doc/man/index.html)、[NSClient++ check_process](https://nsclient.org/docs/scenarios/service-monitoring)、[Sensu process-checks](https://sensu.io/resources/plugins)、[Zabbix proc.num](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ✅ `crates/custos-probe/src/process.rs:10`（工厂 `factory.rs:29`） | 高（已有） | — |
| 受控命令执行（exec） | [Sensu check 执行模型](https://docs.sensu.io/sensu-go/6.1/operations/monitor-sensu/monitor-sensu-with-sensu)、NRPE/check_by_ssh 模式 | ✅ `crates/custos-probe/src/exec.rs:69`（工厂 `factory.rs:37`） | 高（已有） | — |
| systemd unit 状态（active/substate） | [Zabbix systemd.unit.info](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)、[NSClient++ check_service（Linux systemd）](https://nsclient.org/docs/scenarios/service-monitoring) | ❌ 无（`service` 类型仅委托 exec，`factory.rs:42`） | 高 | 直接读 `systemctl show -p ActiveState,SubState` 或调 D-Bus org.freedesktop.systemd1，输出归一化为 Healthy/Unhealthy |
| Windows 服务状态（Started/Stopped、自动启动未运行告警） | [NSClient++ check_service](https://nsclient.org/docs/scenarios/service-monitoring)、[check_nt SERVICESTATE](https://support.nagios.com/kb/print-775.html)、[Nagios XI Windows 方案](https://www.nagios.com/solutions/Windows-monitoring) | ❌ 无（同上委托 exec） | 高 | `sc query <name>` 或 Windows API QueryServiceStatus；比对 StartType=auto 且 state≠RUNNING 判 Unhealthy |
| Docker 容器状态（running/重启次数） | [Uptime Kuma Docker Containers](https://github.com/louislam/uptime-kuma)、[Sensu Docker 插件](https://sensu.io/resources/plugins)、[Centreon Docker](https://github.com/centreon/centreon-plugins) | ❌ 无 | 高 | 复用 ExecProbe 跑 `docker inspect -f '<Go 模板: State.Status>' <name>`，或走 /var/run/docker.sock 的 HTTP API（GET /containers/{id}/json） |
| 进程级资源占用（CPU/内存/句柄数阈值） | [Zabbix proc.mem / proc.cpu.util](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)、[check_procs 内存阈值](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 中 | Linux 读 `/proc/<pid>/stat{,m}` 换算；作为 process 探针的可选阈值参数扩展 |
| Windows 事件日志（error/critical 条目数、event id 过滤） | [NSClient++ CheckEventLog](https://nsclient.org/docs/reference/windows/CheckEventLog)、[Nagios XI 事件日志向导](https://www.nagios.com/solutions/Windows-monitoring) | ❌ 无 | 中 | Windows 用 Windows EID/`wevtutil qe` + 时间窗与 level 过滤；归一化为「时间窗内 error 条数」 |

## 26.3 HTTP / TCP / 端口

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| HTTP(S) 状态码/响应时间 | [check_http](https://www.monitoring-plugins.org/doc/man/index.html)、[blackbox_exporter http prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Uptime Kuma HTTP(s)](https://github.com/louislam/uptime-kuma)、[Zabbix net.tcp.service[https]](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无原生（`http-endpoint` 类型仅委托 exec，`factory.rs:39`） | 高 | reqwest（关闭重定向可选），校验 status∈白名单 + 响应时间阈值；复用 exec.rs 的超时模型 |
| HTTP 关键字/正则匹配 | [Uptime Kuma HTTP(s) Keyword](https://github.com/louislam/uptime-kuma)、[check_http -r/-s](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins)、[blackbox fail_if_matches_regexp](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md) | ❌ 无 | 高 | 与上同一次请求内做 body 正则 contains/matches 断言，避免二次请求 |
| HTTP JSON 断言（json path/值比较） | [Uptime Kuma HTTP(s) Json Query](https://github.com/louislam/uptime-kuma) | ❌ 无 | 中 | serde_json + 简化 dot-path 取值比较（`$.status == "ok"`） |
| TCP 端口连通 | [check_tcp](https://www.monitoring-plugins.org/doc/man/index.html)、[blackbox tcp prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Uptime Kuma Port](https://github.com/louislam/uptime-kuma)、[Zabbix net.tcp.port](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无（`network-connectivity` 类型仅委托 exec，`factory.rs:38`） | 高 | `TcpStream::connect_timeout`，记录连接耗时；可选 starttls/TLS 握手 |
| ICMP Ping（丢包/时延） | [check_ping / check_fping](https://www.monitoring-plugins.org/doc/man/index.html)、[blackbox icmp prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Uptime Kuma Ping](https://github.com/louislam/uptime-kuma) | ❌ 无 | 高 | 用 `servo`/`ping` crate（注意 raw socket 权限，Windows 需 IcmpSendEcho API 或降级为 TCP ping） |
| 协议握手（ssh/smtp/imap/pop/ldap/ftp banner） | [check_ssh / check_smtp / check_imap / check_pop / check_ldap / check_ftp](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix net.tcp.service 服务清单](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 中 | 先 TCP 连接再读 greeting/发探测串匹配期望应答；先做 ssh+smtp 两个最常用的 |
| gRPC 健康检查（grpc.health.v1） | [blackbox grpc prober](https://github.com/prometheus/blackbox_exporter/blob/master/README.md)、[Uptime Kuma gRPC](https://github.com/louislam/uptime-kuma) | ❌ 无 | 中 | tonic + 标准 health proto，Invoke Health check 期望 SERVING |
| UDP 服务探测 | [check_udp](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix net.udp.service](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 中 | 发送 payload + 期望响应正则 + 超时；DNS/NTP 可作专用探针代替 |
| WebSocket 握手 | [blackbox websocket prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Uptime Kuma Websocket](https://github.com/louislam/uptime-kuma) | ❌ 无 | 低 | tokio-tungstenite 完成握手即可判定存活，可选发送/期待消息 |
| MQTT 连接 | Uptime Kuma 支持 MQTT 监控（[社区转述](https://www.facebook.com/thenewstack/posts/uptime-kuma-can-monitor-several-services-including-ping-https-mysql-tcp-port-smt/1749382069827313/)，官方 README 未逐条列出，待核） | ❌ 无 | 低 | rumqttc 连接 + subscribe 主题成功即 Healthy |
| RADIUS 认证探测 | [check_radius](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 低 | Access-Request 包 + 期望 Access-Accept；使用率低，可留给 exec 兜底 |

## 26.4 文件

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| 文件存在/年龄/大小阈值 | [check_file_age](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 中 | 元数据检查：exists + mtime 年龄上限 + size 下限，纯 std 无外部依赖 |
| 日志文件关键字监控（新增行匹配 error） | [check_log](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix log/log.count/logrt](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 高 | 记录 offset 状态，增量读新行做正则匹配，命中即 Unhealthy（自愈引擎里“日志里出现 panic/OOM”是核心触发信号）；轮转按 inode 检测 |

## 26.5 磁盘 / 系统指标

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| 磁盘空间（百分比/绝对值阈值） | [check_disk](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix vfs.fs.size](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)、[Sensu disk-checks](https://sensu.io/resources/plugins)、[Nagios Disk Space Checks](https://support.nagios.com/kb/category.php?id=186) | ❌ 无 | 高 | Linux `statvfs`（nix crate），Windows `GetDiskFreeSpaceExW`；挂载点 + warn/crit 双阈值 |
| inode 使用率 | [Sensu check-disk-usage -K/-W](https://github.com/sensu-plugins/sensu-plugins-disk-checks)、[check_disk](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 低 | 与磁盘空间探针同函数返回 files/ffree，加 inode 阈值参数即可 |
| 内存使用（含 available 语义） | [Sensu memory-checks](https://sensu.io/resources/plugins)、[Zabbix vm.memory.size](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)、[check_ram（Nagios 库）](https://support.nagios.com/kb/category.php?id=186) | ❌ 无 | 高 | Linux 读 `/proc/meminfo` 的 MemAvailable；Windows GlobalMemoryStatusEx |
| CPU 使用率 / 负载 | [check_load](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix system.cpu.util / system.cpu.load](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 高 | 负载：读 `/proc/loadavg` 与核数比值；CPU%：两次采样 /proc/stat 差分 |
| swap 使用 | [check_swap](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 中 | Linux `/proc/meminfo` SwapTotal/SwapFree；随内存探针一并实现成本低 |
| NTP 时钟偏移 | [check_ntp_time / check_ntp_peer](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 中 | 简单 SNTP 客户端（UDP 123，一次往返算 offset），阈值告警 |
| 登录用户数 | [check_users](https://www.monitoring-plugins.org/doc/man/index.html)、[Zabbix system.users.num](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 低 | 解析 `utmp` 或跑 `who \| wc -l`，场景有限可后置 |
| SMART/硬件传感器 | [check_ide_smart / check_sensors](https://www.monitoring-plugins.org/doc/man/index.html)、[Sensu check-smart](https://github.com/sensu-plugins/sensu-plugins-disk-checks)、[Centreon Hardware 类连接器](https://www.centreon.com/connectors/) | ❌ 无 | 低 | 委托 `smartctl -H` 输出判 OK；硬件传感器走 lm-sensors 输出解析 |
| UPS 状态 | [check_ups](https://www.monitoring-plugins.org/doc/man/index.html)、[Centreon UPS-PDU 类](https://www.centreon.com/connectors/) | ❌ 无 | 低 | 走 NUT 协议（upsmon/upsrw）判定 OB/LB 状态 |

## 26.6 数据库

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| MySQL/MariaDB 连通 + 轻量查询 | [check_mysql / check_mysql_query](https://www.monitoring-plugins.org/doc/man/index.html)、[Uptime Kuma MySQL](https://github.com/louislam/uptime-kuma)、[Centreon MySQL/MariaDB 连接器](https://www.centreon.com/connectors/) | ❌ 无 | 中 | mysql_async 执行 `SELECT 1` + 可选阈值查询；连接失败/超时分别归一化 |
| PostgreSQL 连通 + 查询 | [check_pgsql](https://www.monitoring-plugins.org/doc/man/index.html)、[Uptime Kuma PostgreSQL](https://github.com/louislam/uptime-kuma)、[Centreon PostgreSQL](https://www.centreon.com/connectors/) | ❌ 无 | 中 | tokio-postgres `SELECT 1`；可扩展 `pg_is_in_recovery()` 等只读断言 |
| Redis 连通 | [Centreon Redis](https://github.com/centreon/centreon-plugins)、[Uptime Kuma Redis](https://www.facebook.com/thenewstack/posts/uptime-kuma-can-monitor-several-services-including-ping-https-mysql-tcp-port-smt/1749382069827313/) | ❌ 无 | 中 | redis crate 发 `PING` 期待 `PONG`，可选 INFO 指标阈值 |
| 其它数据库（MSSQL/Oracle/Mongo） | [check_mssql / check_oracle / check_dbi](https://www.monitoring-plugins.org/doc/man/index.html)、[Centreon Databases 分类](https://github.com/centreon/centreon-plugins) | ❌ 无 | 低 | 长尾，先用 ExecProbe 兜底（`sqlcmd`/`psql` 等），有需求再原生 |

## 26.7 容器 / 虚拟化

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| Docker 容器状态 | 见 26.2 行 | ❌ 无 | 高 | 见 26.2 行 |
| Kubernetes 工作负载（pod/deployment 就绪） | 未找到公开资料（待核；Centreon/Zabbix 均有 K8s 方案但本次未取证） | ❌ 无 | 低 | kube API GET /apis/apps/v1/namespaces/ns/deployments 比对 readyReplicas；依赖面大，最后做 |

## 26.8 证书 / DNS

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| TLS 证书到期/证书链校验 | [check_http -D（证书到期）](https://github.com/nagios-plugins/nagios-plugins/tree/master/plugins)、[blackbox probe_ssl_oui/证书链校验](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Sensu HTTP 插件 SSL expiry](https://sensu.io/resources/plugins) | ❌ 无 | 高 | rustls/native-tls 拿 peer 证书 not_after，剩余天数阈值（<30 warn，<7 crit 是业界惯例）；不依赖 HTTP 探针可对任意 host:port 做 |
| DNS 记录解析（A/AAAA/CNAME + 应答匹配） | [check_dns / check_dig](https://www.monitoring-plugins.org/doc/man/index.html)、[blackbox dns prober](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)、[Uptime Kuma DNS Record](https://github.com/louislam/uptime-kuma)、[Zabbix net.dns.record](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent) | ❌ 无 | 高 | hickory-resolver（trust-dns）指定 resolver server + record type + 应答正则断言 |
| DoT/DoH | [blackbox dns_over_tls](https://github.com/prometheus/blackbox_exporter/blob/master/CONFIGURATION.md)（DoT）；DoH 未找到公开资料（待核） | ❌ 无 | 低 | DoT：TLS 853 端口复用 DNS 探针；DoH 后置 |

## 26.9 网络设备 / 基础网络

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| SNMP OID 采集/阈值 | [check_snmp](https://www.monitoring-plugins.org/doc/man/index.html)、[Centreon 700+ SNMP 连接器](https://www.centreon.com/connectors/)、[Uptime Kuma SNMP](https://github.com/louislam/uptime-kuma) | ❌ 无 | 中 | snmp crate（v2c/v3），OID 采样 + 阈值比较；网络设备场景的事实标准 |
| 接口状态/流量（up/down、误码） | [Zabbix net.if.in / net.if.discovery](https://zabbix.com/documentation/current/en/manual/config/items/itemtypes/zabbix_agent)、[check_ifstatus / check_ifoperstatus](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 中 | 本机：读 `/sys/class/net/<if>/operstate` + statistics；远端：SNMP ifTable |
| OpenWrt/UBUS 探测面（network.interface status、service list、device 状态） | [OpenWrt ubus：`ubus call network.interface.wan status`、`ubus call service list`](https://openwrt.org/docs/techref/ubus) | ❌ 无 | 中 | custos 若面向软路由/网关场景：通过 ubus socket（/var/run/ubus.sock）JSON-RPC 调 `network.interface.<name>.status` 的 `up` 字段与 `service.list`；与 docs/guide/probe-diagnosis.md §5.1 的 Wi-Fi/IP/网关探针链天然衔接 |
| DHCP 服务探测 | [check_dhcp](https://www.monitoring-plugins.org/doc/man/index.html) | ❌ 无 | 低 | 发 DHCPDISCOVER 广播等 OFFER；需要 raw socket 权限，长尾 |

## 26.10 其它

| 探测类型 | 业界谁在用 | custos 现状 | 优先级 | 实现要点建议（未实现） |
|---|---|---|---|---|
| VPN/隧道端到端验证（不只是端口通，而是经隧道取到预期内容） | 无单一标准实现（check_http 经代理模式 `-I proxy` 最接近，[check_http 手册](https://www.monitoring-plugins.org/doc/man/index.html)） | ⚠️ `tunnel`/`proxy` 类型存在但仅委托 exec（`factory.rs:40-41`），无原生语义 | 中 | 原生化：隧道资源上跑"通过隧道发起 HTTP/TCP 请求并断言出口 IP/目标可达"，把探测面与自愈动作（重拨/重启隧道）对齐 |
| Push/主动心跳（被监控端主动上报，超时判死） | [Uptime Kuma Push](https://github.com/louislam/uptime-kuma) | ❌ 无 | 低 | 自愈引擎多为主动探测模型，心跳模型引入状态存储；按需后置 |
| 游戏服务器（Steam/Source） | [Uptime Kuma Steam Game Server](https://github.com/louislam/uptime-kuma) | ❌ 无 | 低 | A2S 查询协议，长尾 |

## 26.11 建议实现顺序

依据：自愈引擎需要**零插件就能覆盖最常见的自愈触发信号**；先做"高频故障 + 实现成本低"的，再做依赖重的。

1. **HTTP(S) 端点探测**（状态码 + 关键字 + 响应时间）——最常见的存活/质量信号；一次请求覆盖三件事（26.3 前两行）。
2. **TCP 端口连通** —— 最小实现（connect_timeout），是网络类诊断链的基元。
3. **DNS 解析** —— 与 TCP/HTTP 组成完整网络诊断链（对应 probe-diagnosis.md §5.1 的证据链）。
4. **TLS 证书到期** —— 复用 2/3 的 TLS 能力，纯阈值逻辑，自愈价值高（提前告警）。
5. **磁盘空间 / 内存 / CPU 负载**（系统三件套，同一批实现，共享阈值模型）。
6. **日志文件关键字监控**（增量 offset + 正则）——"日志出现 panic/OOM"是自愈引擎的高价值触发信号。
7. **systemd unit 状态**（Linux `service` 类型的原生实现，替代委托 exec 的默认路径）。
8. **Docker 容器状态** —— 容器化部署普及度高，实现走 docker.sock 或复用 ExecProbe。
9. **ICMP Ping** —— 注意权限降级路径（无 raw socket 时 TCP ping 兜底）。
10. **文件存在/年龄 + swap**（低成本补齐系统域）。
11. **Windows 服务状态 + 事件日志**（有 Windows 部署需求时；custos exec/process 已有 Windows 分支，见 process.rs 的 tasklist/taskkill 路径）。
12. **MySQL/PostgreSQL/Redis 连通**（数据库域按用户实际栈挑选）。
13. **SNMP 采集 + 接口状态**（网络设备场景）。
14. **OpenWrt/UBUS 探测面**（若软路由/网关是目标场景则提前到 3 之后）。
15. 其余（gRPC/WebSocket/MQTT/UPS/SMART/K8s/Push 等）按需求长尾，继续由 ExecProbe 兜底。

---

*本文档由 custos 探测能力调研生成；custos 代码证据基于 `crates/custos-probe`（process.rs 117 行 / exec.rs 338 行 / factory.rs 90 行）。外部工具能力以所附链接为准。*
