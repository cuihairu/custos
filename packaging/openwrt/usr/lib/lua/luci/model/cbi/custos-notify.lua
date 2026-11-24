-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Notification settings page (C-SET-UX-03). The former "启用通知" flag is
-- not an on/off switch for notifications: per the engine's env overlay
-- (crates/custos-config notification::env_overlay) the LuCI channel rules
-- REPLACE the TOML [notification] section wholesale when the flag is on and
-- at least one complete channel exists, and the TOML rules stand otherwise.
-- This page names the override as such, shows which source is currently in
-- effect, validates the Telegram pair at the editor (init.d silently drops
-- a half-set pair), and never echoes token values into previews or JSON.

local settings = require "luci.custos.settings"

m = Map("custos", translate("Custos 通知设置"),
	translate("先确认当前规则来自哪里，再配置发往哪些渠道。通知阈值与规则在监控配置文件 [notification] 段也有独立定义。"))

-- Channel changes reach the engine as CUSTOS_NOTIFY_* environment vars,
-- which only re-read on start; restart after a successful apply.
function m.on_commit()
	luci.sys.call("/etc/init.d/custos restart >/dev/null 2>&1")
end

n = m:section(NamedSection, "notify", "notify", translate("LuCI 通知渠道"))
n.addremove = false

-- The override flag, named for what it does (C-SET-UX-03: the old
-- "启用通知" wording was read as "stop all notifications when off").
o = n:option(Flag, "enabled", translate("用 LuCI 覆盖通知配置"))
o.rmempty = false
o.description = translate(
	"打开且至少启用一个完整渠道（本地日志勾选 / Telegram 两项成对 / Herald 地址）后，LuCI 渠道整体替换监控文件的通知段；关闭或无完整渠道时恢复按 TOML 执行")

-- Which source is effective right now, computed from the stored UCI values
-- with the same rule the engine applies to its environment variables.
d = n:option(DummyValue, "_effective_source", translate("当前谁在生效"))
function d.cfgvalue(self, section)
	local function get(key)
		return m.uci:get("custos", section, key)
	end
	return settings.source_note(get("enabled"), get("local_log"),
		get("telegram_token"), get("telegram_chat"), get("herald_url"), false)
end

o = n:option(ListValue, "min_severity", translate("最低通知级别"))
o:value("info", "info")
o:value("warning", "warning")
o:value("critical", "critical")
o.default = "warning"
o.description = translate("仅覆盖生效时使用；TOML 规则自带各自的级别阈值")

o = n:option(Flag, "local_log", translate("本地日志渠道"))
o.description = translate("通知同时写入本地审计日志（audit log）")

-- Values travel through the environment; whitespace would break the
-- procd env word-splitting, so every free-text field rejects it.
local function no_space(self, value)
	if value and value:find("%s") then
		return nil, translate("不能包含空白字符")
	end
	return value
end

-- Telegram pair validation at the editor: read the sibling field from the
-- same submit and refuse a half-filled channel, instead of letting init.d
-- drop it silently (which would read as "channel not configured").
local function telegram_pair(other_key)
	return function(self, value)
		local other = luci.http.formvalue("cbid.custos.notify." .. other_key) or ""
		value = value or ""
		local err = settings.telegram_pair_error(value, other)
		if err then
			return nil, translate(err)
		end
		return no_space(self, value)
	end
end

o = n:option(Value, "telegram_token", translate("Telegram Bot Token"))
o.password = true
o.validate = telegram_pair("telegram_chat")
o.description = translate("与接收会话 ID 成对填写；预览与诊断页不显示令牌")

o = n:option(Value, "telegram_chat", translate("Telegram 接收会话 ID"))
o.validate = telegram_pair("telegram_token")
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
o.description = translate("herald 关闭鉴权时可留空；预览与诊断页不显示令牌")

return m
