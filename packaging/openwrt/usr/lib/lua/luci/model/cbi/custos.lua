-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

m = Map("custos", translate("Custos 自愈引擎"),
	translate("跨平台本地故障检测与自动恢复引擎。探测资源并按恢复策略自动处理故障。"))

-- Restart the procd service after a successful apply so enabled/db/conf/
-- interval/notification changes take effect immediately (the init script
-- re-reads UCI on start).
function m.on_commit()
	luci.sys.call("/etc/init.d/custos restart >/dev/null 2>&1")
end

-- 全局设置：服务开关与运行参数。
s = m:section(NamedSection, "main", "custos", translate("全局设置"))
s.addremove = false

o = s:option(Flag, "enabled", translate("启用服务"))
o.rmempty = false

o = s:option(Value, "db", translate("数据库路径"))
o.default = "/etc/custos/custos.db"
o.rmempty = false

o = s:option(Value, "conf", translate("监控配置文件路径"))
o.default = "/etc/custos/custos.toml"
o.rmempty = false
o.description = translate("资源探针与恢复策略的 TOML 配置文件（init.d 用它启动引擎）")

o = s:option(Value, "interval", translate("探测间隔 (ms)"))
o.default = "5000"
o.datatype = "uinteger"

-- 通知设置：最小字段经 CUSTOS_NOTIFY_* 环境变量透传给引擎（init.d 渲染），
-- 任何通道选择变量存在时引擎用该覆盖整体替换 TOML [notification] 段
-- （custos-config notification::env_overlay），不重写配置文件。
n = m:section(NamedSection, "notify", "notify", translate("通知设置"))
n.addremove = false

o = n:option(Flag, "enabled", translate("启用通知"))
o.rmempty = false
o.description = translate(
	"开启后 init.d 经环境变量把下列通道透传给引擎；关闭则完全按 TOML 配置文件的通知段执行")

o = n:option(Flag, "local_log", translate("本地日志通道"))
o.description = translate("通知同时写入本地审计日志（audit log）")

o = n:option(ListValue, "min_severity", translate("最低通知级别"))
o:value("info", "info")
o:value("warning", "warning")
o:value("critical", "critical")
o.default = "warning"

-- Values travel through the environment; whitespace would break the
-- procd env word-splitting, so the secret/url fields reject it.
local function no_space(self, value)
	if value and value:find("%s") then
		return nil, translate("不能包含空白字符")
	end
	return value
end

o = n:option(Value, "telegram_token", translate("Telegram Bot Token"))
o.password = true
o.validate = no_space
o.description = translate("与 Chat ID 需同时填写（只填其一会按未配置处理）")

o = n:option(Value, "telegram_chat", translate("Telegram Chat ID"))
o.validate = no_space
o.description = translate("数字 ID、@channelname 或频道用户名")

o = n:option(Value, "herald_url", translate("Herald 服务地址"))
o.validate = no_space
o.description = translate("如 http://127.0.0.1:8080（须 http/https）")

o = n:option(Value, "herald_app", translate("Herald 应用 ID"))
o.default = "custos"
o.validate = no_space

o = n:option(Value, "herald_token", translate("Herald Token"))
o.password = true
o.validate = no_space
o.description = translate("herald 关闭鉴权时可留空")

return m
