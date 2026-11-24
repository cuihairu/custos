-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Service settings page (C-SET-UX-01): only the UCI-owned runtime section.
-- Monitoring resources and recovery policies live in the TOML file and are
-- edited under 监控资源; notification channels live under 通知设置. Keeping
-- the sections on separate pages is what makes the config source visible:
-- saving here only ever writes custos.main.* UCI keys.

local settings = require "luci.custos.settings"

m = Map("custos", translate("Custos 服务设置"),
	translate("控制 OpenWrt 上的 Custos 服务运行参数（UCI）。资源、依赖和恢复策略来自监控配置文件，请在「监控资源」页编辑。"))

-- Restart the procd service after a successful apply so enabled/db/conf/
-- interval changes take effect immediately (the init script re-reads UCI
-- on start).
function m.on_commit()
	luci.sys.call("/etc/init.d/custos restart >/dev/null 2>&1")
end

s = m:section(NamedSection, "main", "custos", translate("服务运行参数"))
s.addremove = false

o = s:option(Flag, "enabled", translate("启用服务"))
o.rmempty = false
o.description = translate("关闭后停止探测、恢复与通知处理")

-- The engine takes whole milliseconds (`custos start --interval <ms>`),
-- while the prototype and users think in seconds. Show seconds, keep
-- millisecond precision: 1.5 s round-trips to the UCI value "1500".
o = s:option(Value, "interval", translate("引擎检查周期（秒）"))
o.rmempty = false
o.description = translate("全局引擎循环周期；各资源的探测间隔在「监控资源」中单独设置。支持毫秒精度，例如 1.5 秒")
function o.validate(self, value)
	local ms = settings.to_ms(value or "")
	if not ms then
		return nil, translate("须为正数，秒为单位、精确到毫秒（例如 1.5）")
	end
	return settings.to_seconds(ms)
end
function o.cfgvalue(self, section)
	local ms = m.uci:get("custos", section, "interval")
	return ms and settings.to_seconds(ms) or nil
end
function o.write(self, section, value)
	local ms = settings.to_ms(value or "")
	if ms then
		m.uci:set("custos", section, "interval", tostring(ms))
	end
end

o = s:option(Value, "conf", translate("监控配置文件路径"))
o.default = "/etc/custos/custos.toml"
o.rmempty = false
o.description = translate("资源探针与恢复策略的 TOML 配置文件（init.d 用它启动引擎；编辑入口在「监控资源」页）")

o = s:option(Value, "db", translate("状态数据库路径"))
o.default = "/etc/custos/custos.db"
o.rmempty = false
o.description = translate("保存恢复记录、通知队列及审计")

-- Config source legend (C-SET-UX-01): state on the page which store each
-- kind of setting comes from, instead of letting the user guess.
d = s:option(DummyValue, "_source_legend", translate("配置来源"))
d.rawhtml = false
function d.cfgvalue(self, section)
	return "运行参数 = OpenWrt UCI（本页） · 资源与恢复策略 = Custos TOML（监控资源页） · 通知渠道 = UCI 覆盖或 TOML（通知设置页）"
end

return m
