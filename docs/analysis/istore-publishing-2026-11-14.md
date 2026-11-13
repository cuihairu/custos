# iStore 上架要求调研（2026-11-14）

目的：明确 custos 进 iStore（iStoreOS 应用商店）需要什么格式，并让仓库里的 OpenWrt 打包达到「随时可提交」状态。

## 结论速览

1. iStore 应用 = 一组普通 opkg 包，其中必须有一个 `app-meta-<name>` 元数据包（图标 + JSON 清单），商店面板靠它展示应用。
2. 本仓库已产出该包（`app-meta-custos`，由 `packaging/openwrt/build-ipk.sh` 构建），清单格式对照真机已装包逐字段核实。
3. 正式进商店列表需联系 iStore（Linkease）方提交；公开仓库规格文档本次未能核到（GitHub 从本机不可达），**提交渠道以官方回复为准**。
4. 包本体同时满足通用 opkg 规范（control 字段齐全、conffiles、幂等 maintainer 脚本），纯 `opkg install` 不依赖商店也可用。

## 可核实的来源（本机网络可达部分）

- https://doc.istoreos.com/zh/guide/istore/ — iStore 使用说明（面向用户，不含上架规格）。
- https://doc.istoreos.com/zh/istore/post-my-tutorial.html — 仅「发表使用教程」页，无开发者上架规格。
- 官方联系邮箱：admin@linkease.com（doc 站页脚）。
- **GitHub 不可达**：raw.githubusercontent.com HTTP 000、`gh api` EOF、`git clone`（linkease 仓库）连接被关闭，镜像代理亦为空。linkease 的 app-meta 仓库规格本次未能直接核对，故规格以下述「真机一手核实」为准，提交渠道待官方确认。

## 规格（真机一手核实，iStoreOS @ 192.168.5.11）

### app-meta 包形态

- 包名：`app-meta-<name>`，`Architecture: all`，Depends 链指向真正的 LuCI 应用包（实机例：`app-meta-lan-wake` Depends `libc, luci-app-lan-wake, luci-js-deps`）。
- 必带文件：
  - `/usr/lib/opkg/meta/<name>.json` — 应用清单；
  - `/www/luci-static/resources/app-icons/<name>.png` — 应用图标。
- 可选钩子：`/usr/libexec/istorea/<name>.sh`、`/usr/libexec/istoree/<name>.sh`（实机见 aria2/jellyfin/linkease/openclawmgr 等；custos 暂不需要）。

### 清单 JSON 字段（实机例 lan-wake.json + 其余已装包对照）

```json
{
  "name": "lan-wake",
  "title": "局域网设备唤醒",
  "title_en": "Wake-on-LAN",
  "entry": "/cgi-bin/luci/admin/services/lan-wake",
  "author": "...",
  "website": "https://github.com/...",
  "version": "1.1.0",
  "release": 1,
  "arch": ["all"],
  "description": "...",
  "description_en": "...",
  "tags": ["net","tool"],
  "depends": ["luci-app-lan-wake","luci-js-deps"]
}
```

- `tags` 实机可见词表：`net`、`tool`、`multimedia`、`download`、`ai`。
- `arch`：纯 LuCI 应用用 `["all"]`；custos 清单声明 `["x86_64","aarch64"]`（与 nightly 出包矩阵一致）。
- `entry`：LuCI 页面路径；custos 用 `/cgi-bin/luci/admin/services/custos`。

### custos 的对应产出

- `app-meta-custos.json`（`packaging/openwrt/app-meta/custos.json`）：中英双语 title/description、entry 指向 LuCI 页、tags `["net","tool"]`、depends `["custos","luci-app-custos"]`。
- 图标：`assets/icons/custos.png`（128×128，从 ico 转换）。
- 安装顺序：`custos` → `luci-app-custos` → `app-meta-custos`（Depends 链即此序）；`opkg install app-meta-custos_*.ipk` 可一次拉全。

## 通用 opkg 合规（不依赖商店也能装）

- `CONTROL/control` 字段齐全：Package/Version/Depends/Source/SourceName/License/Section/SourceDateEpoch/Maintainer/Architecture/Installed-Size/Description（英文行 + 中文续行）。
- conffiles：`/etc/config/custos`、`/etc/custos/custos.toml`、`/etc/init.d/custos`。真机版本升级实测：改动过的 custos.toml 保留（md5 不变），新默认落为 `custos.toml-opkg`。
- postinst/prerm 幂等、**0755 权限**（opkg 直接执行，非 755 会导致 configure 报 exit 126）；luci-app postinst 清 `/tmp/luci-indexcache*` 让新路由即时生效。
- 多架构：`build-ipk.sh --arch x86_64|aarch64`；nightly 矩阵已含两条 musl 腿（x86_64 交叉、aarch64 原生）。

## 待办（对外提交）

- [ ] 联系 Linkease（admin@linkease.com）确认上架提交流程（邮件 / openwrt-app-meta 仓库 PR / 其他），GitHub 可达后先核对该仓库规格再提交。
- [ ] 按 iStore 方要求补齐商店侧材料（截图、教程链接等，当前规格文档未给出清单）。
