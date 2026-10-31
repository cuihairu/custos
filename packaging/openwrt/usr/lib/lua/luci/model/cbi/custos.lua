-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

m = Map("custos", translate("Custos 自愈引擎"),
	translate("跨平台本地故障检测与自动恢复引擎。探测资源并按恢复策略自动处理故障。"))

-- Restart the procd service after a successful apply so enabled/db/interval
-- changes take effect immediately (the init script re-reads UCI on start).
function m.on_commit()
	luci.sys.call("/etc/init.d/custos restart >/dev/null 2>&1")
end

s = m:section(NamedSection, "main", "custos", translate("全局设置"))
s.addremove = false

o = s:option(Flag, "enabled", translate("启用服务"))
o.rmempty = false

o = s:option(Value, "db", translate("数据库路径"))
o.default = "/etc/custos/custos.db"
o.rmempty = false

o = s:option(Value, "interval", translate("探测间隔 (ms)"))
o.default = "5000"
o.datatype = "uinteger"

-- TOML config file path (read-only hint; the TOML is edited manually)
o = s:option(DummyValue, "_toml", translate("TOML 探针配置"))
o.value = "/etc/custos/custos.toml"
o.rawhtml = true

return m
