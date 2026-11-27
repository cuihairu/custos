-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Pure helpers shared by the Custos LuCI settings pages. Kept free of any
-- luci.* require so the unit tests (packaging/openwrt/tests/settings-test.lua)
-- can run on host Lua and on the device.
--
-- Three concerns, each mirroring an engine-side contract:
--   * seconds <-> milliseconds conversion for the UCI "interval" key. The
--     init script passes the stored value verbatim as `custos start
--     --interval <ms>` (etc/init.d/custos), so UCI keeps storing whole
--     milliseconds while the form speaks seconds with millisecond precision.
--   * the Telegram pair rule: the engine's env overlay treats token+chat as
--     one channel, and init.d notify_pairs() drops a half-set pair, so a
--     half-filled form must be caught at the editor instead of silently
--     meaning "no channel" (crates/custos-config notification::env_overlay).
--   * the Herald URL rule: the engine rejects a non-http/https address by
--     failing the whole config load, and the env overlay hardcodes the
--     default SSRF policy (loopback/private blocked, no allowlist), so a
--     literal blocked target must be refused at the editor as well.
--   * the effective notification source: the overlay REPLACES the TOML
--     [notification] section only when the override flag is on AND at least
--     one complete channel selector exists; otherwise the TOML section
--     stands. The settings page must say which one is in effect.

local M = {}

-- Parse a seconds figure ("5", "1.5", "0.001") into whole milliseconds.
-- Returns an integer >= 1, or nil when the text is not a positive number or
-- carries a residue finer than one millisecond (0.0004 s would round to 0).
function M.to_ms(text)
	if type(text) ~= "string" then
		return nil
	end
	local s = text:match("^%s*(.-)%s*$")
	if s == "" or not s:match("^%d+%.?%d*$") then
		return nil
	end
	local whole, frac = s:match("^(%d+)%.?(%d*)$")
	local ms = tonumber(whole) * 1000
	if frac and frac ~= "" then
		-- Millisecond resolution: keep three fractional digits, refuse more.
		if #frac > 3 then
			return nil
		end
		while #frac < 3 do
			frac = frac .. "0"
		end
		ms = ms + tonumber(frac)
	end
	if ms < 1 then
		return nil
	end
	return ms
end

-- Render milliseconds as the canonical seconds string the form shows
-- ("5000" -> "5", "1500" -> "1.5", "1" -> "0.001"). Non-positive or
-- non-numeric input yields nil; the callers guard against it.
function M.to_seconds(ms)
	local n = tonumber(ms)
	if not n or n < 1 or math.floor(n) ~= n then
		return nil
	end
	local whole = math.floor(n / 1000)
	local frac = n % 1000
	if frac == 0 then
		return tostring(whole)
	end
	local text = tostring(frac)
	while #text < 3 do
		text = "0" .. text
	end
	-- Trim trailing zeros so 1500 -> "1.5" instead of "1.500".
	text = text:gsub("0+$", "")
	return whole .. "." .. text
end

-- Mirrors the two Herald target rules the env overlay enforces at engine
-- start: an http/https scheme (a non-http scheme fails the whole config
-- load), and SsrfPolicy::default_blocking() with an empty allowlist (a
-- loopback/private target makes the channel fail to build, so every event
-- dead-letters while the page still claims LuCI is in effect). Literal
-- blocked addresses and localhost are refused here; hostnames are left to
-- the engine's resolve-then-check, which has the final say.
local function blocked_ipv4(a, b, c, d)
	a, b, c, d = tonumber(a), tonumber(b), tonumber(c), tonumber(d)
	if not (a and b and c and d) then
		return false
	end
	if a == 0 and b == 0 and c == 0 and d == 0 then
		return true
	end
	if a == 10 or a == 127 then
		return true
	end
	if a == 172 and b >= 16 and b <= 31 then
		return true
	end
	if a == 192 and b == 168 then
		return true
	end
	if a == 169 and b == 254 then
		return true
	end
	-- CGNAT (100.64.0.0/10) is blocked by the engine's is_cgnat check.
	if a == 100 and b >= 64 and b <= 127 then
		return true
	end
	return false
end

-- Expand an IPv6 literal into its eight 16-bit groups, or nil when the text
-- is not one. A trailing dotted quad (the IPv4-mapped form) becomes two
-- groups first, matching how the engine normalises it before checking.
local function ipv6_groups(host)
	local quad = host:match("(%d+%.%d+%.%d+%.%d+)$")
	if quad then
		local a, b, c, d = quad:match("^(%d+)%.(%d+)%.(%d+)%.(%d+)$")
		a, b, c, d = tonumber(a), tonumber(b), tonumber(c), tonumber(d)
		if not (a and b and c and d) or a > 255 or b > 255 or c > 255 or d > 255 then
			return nil
		end
		host = host:sub(1, #host - #quad) .. string.format("%x:%x", a * 256 + b, c * 256 + d)
	end
	local function push(part, out)
		if part == "" then
			return true
		end
		for group in part:gmatch("[^:]+") do
			local value = tonumber(group, 16)
			if not value or value > 0xffff then
				return false
			end
			out[#out + 1] = value
		end
		return true
	end
	local head, tail = host:match("^(.-)::(.*)$")
	if not head then
		local groups = {}
		if not push(host, groups) or #groups ~= 8 then
			return nil
		end
		return groups
	end
	local left, right = {}, {}
	if not push(head, left) or not push(tail, right) then
		return nil
	end
	local missing = 8 - #left - #right
	if missing < 1 then
		return nil
	end
	for _ = 1, missing do
		left[#left + 1] = 0
	end
	for _, value in ipairs(right) do
		left[#left + 1] = value
	end
	return left
end

local function blocked_ipv6(host)
	local g = ipv6_groups(host)
	if not g then
		return false
	end
	local all_zero = true
	for _, value in ipairs(g) do
		if value ~= 0 then
			all_zero = false
		end
	end
	if all_zero then
		return true -- ::
	end
	if g[8] == 1 and g[1] == 0 and g[2] == 0 and g[3] == 0 and g[4] == 0
		and g[5] == 0 and g[6] == 0 and g[7] == 0 then
		return true -- ::1 loopback
	end
	-- IPv4-mapped (::ffff:a.b.c.d) is checked as IPv4 by the engine.
	if g[1] == 0 and g[2] == 0 and g[3] == 0 and g[4] == 0 and g[5] == 0 and g[6] == 0xffff then
		return blocked_ipv4(math.floor(g[7] / 256), g[7] % 256, math.floor(g[8] / 256), g[8] % 256)
	end
	-- fc00::/7 (unique local) and fe80::/10 (link local).
	if g[1] >= 0xfc00 and g[1] <= 0xfdff then
		return true
	end
	if g[1] >= 0xfe80 and g[1] <= 0xfebf then
		return true
	end
	return false
end

local function blocked_host(host)
	host = host:lower()
	if host == "localhost" then
		return true
	end
	if host:find(":", 1, true) then
		return blocked_ipv6(host)
	end
	return blocked_ipv4(host:match("^(%d+)%.(%d+)%.(%d+)%.(%d+)$"))
end

function M.herald_url_error(value)
	if value == nil or value == "" then
		return nil
	end
	local scheme = value:match("^(.-)://")
	local lower = scheme and scheme:lower() or nil
	if lower ~= "http" and lower ~= "https" then
		return "须以 http:// 或 https:// 开头（否则引擎启动时会拒绝整个配置）"
	end
	local authority = value:match("^%a+://([^/?#]+)") or ""
	authority = authority:match("@([^@]+)$") or authority
	local host = authority:match("^%[(.-)%]") or authority:match("^([^:]+)") or authority
	if blocked_host(host) then
		return "须为公网地址（回环/私网被引擎 SSRF 策略拒绝，无法送达）"
	end
	return nil
end

-- The Telegram channel counts only when bot token and chat id are present
-- together. Returns the editor-level error message, or nil when the pair is
-- complete (both set) or intentionally absent (both empty).
function M.telegram_pair_error(token, chat)
	local has_token = token ~= nil and token ~= ""
	local has_chat = chat ~= nil and chat ~= ""
	if has_token == has_chat then
		return nil
	end
	if has_token then
		return "与接收会话 ID 需同时填写（只填其一会按未配置处理）"
	end
	return "与机器人令牌需同时填写（只填其一会按未配置处理）"
end

-- Which notification source is actually in effect, mirroring the engine's
-- env overlay: LuCI rules win only when the override flag is on and at
-- least one complete channel selector exists (local log on, telegram pair
-- complete, or herald url set). A half-set telegram pair is ignored exactly
-- like init.d drops it. Returns "luci" or "toml".
function M.effective_source(enabled, local_log, tg_token, tg_chat, herald_url)
	if tostring(enabled) ~= "1" then
		return "toml"
	end
	if tostring(local_log) == "1" then
		return "luci"
	end
	if tg_token ~= nil and tg_token ~= "" and tg_chat ~= nil and tg_chat ~= "" then
		return "luci"
	end
	if herald_url ~= nil and herald_url ~= "" then
		return "luci"
	end
	return "toml"
end

-- The human-facing explanation for the effective source. Without `pending`
-- it describes the service right now (computed from UCI); with `pending` it
-- describes what a Save would switch to (computed from submitted fields).
function M.source_note(enabled, local_log, tg_token, tg_chat, herald_url, pending)
	local source = M.effective_source(enabled, local_log, tg_token, tg_chat, herald_url)
	local prefix = pending and "应用后将生效：" or "当前生效："
	if source == "luci" then
		return prefix .. "LuCI 通知规则（引擎读取 CUSTOS_NOTIFY_* 环境变量，整体替换监控文件中的 [notification] 段）"
	end
	if tostring(enabled) == "1" then
		return prefix .. "监控文件 [notification] 段（覆盖已打开，但没有完整渠道：本地日志未勾选、Telegram 两项未成对、Herald 地址为空——半配渠道按未配置处理，不会让引擎报错）"
	end
	return prefix .. "监控文件 [notification] 段（LuCI 覆盖关闭，引擎按 TOML 已有规则投递）"
end

-- Values a Save would apply: submitted form fields win (including an
-- explicit empty string from an unchecked flag), absent fields fall back to
-- the stored UCI values. Token values are only inspected for presence by
-- source_note; this module never renders them.
function M.pending_values(form, stored)
	local values = {}
	for _, key in ipairs({
		"enabled", "local_log", "telegram_token", "telegram_chat", "herald_url"
	}) do
		local value = form and form[key]
		if value == nil then
			value = stored and stored[key]
		end
		values[key] = value
	end
	return values
end

return M
