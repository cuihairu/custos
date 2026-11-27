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
--   * the Herald URL scheme rule: the engine rejects a non-http/https
--     herald address by failing the whole config load, so the editor must
--     refuse it before it ever reaches UCI (same env overlay).
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

-- The Herald channel only accepts an http/https base URL. Returns the
-- editor-level error message, or nil when the value is absent or carries an
-- http/https scheme. Mirrors the engine's env overlay rule verbatim
-- (crates/custos-config notification::env_overlay): there a non-http scheme
-- fails the whole config load, so saving such a value would keep the engine
-- from starting at all.
function M.herald_url_error(value)
	if value == nil or value == "" then
		return nil
	end
	local scheme = value:match("^(.-)://")
	local lower = scheme and scheme:lower() or nil
	if lower ~= "http" and lower ~= "https" then
		return "须以 http:// 或 https:// 开头（否则引擎启动时会拒绝整个配置）"
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

-- The human-facing explanation for the effective source. `uci_effective`
-- describes what the running service uses right now (computed from UCI);
-- `pending` shifts the wording to after-apply when the form is being
-- configured differently from the stored values.
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

return M
