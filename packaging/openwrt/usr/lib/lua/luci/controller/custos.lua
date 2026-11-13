-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

module("luci.controller.custos", package.seeall)

function index()
	if not nixio.fs.access("/etc/config/custos") then
		return
	end

	entry({"admin", "services", "custos"},
		cbi("custos"),
		_("Custos 自愈引擎"), 60).dependent = true

	entry({"admin", "services", "custos", "status"},
		template("custos/status"),
		_("运行状态"), 10).leaf = true

	entry({"admin", "services", "custos", "api", "overview"},
		call("api_overview")).leaf = true
end

-- Strip control characters and cap the length. The page renders through
-- DOM textContent so HTML injection is impossible, but log-derived
-- observations still must not flood the response or the table row.
local function sanitize(value, max)
	if value == nil then
		return ""
	end
	local s = tostring(value):gsub("%c", "")
	if #s > max then
		s = s:sub(1, max) .. "…"
	end
	return s
end

local function shell_quote(s)
	return "'" .. tostring(s):gsub("'", "'\\''") .. "'"
end

local function ms_to_iso(ms)
	local t = tonumber(ms)
	if not t or t <= 0 then
		return nil
	end
	return os.date("%Y-%m-%d %H:%M:%S", math.floor(t / 1000))
end

function api_overview()
	luci.http.prepare_content("application/json")
	local sys = require("luci.sys")
	local utl = require("luci.util")

	-- procd service state via the rc.common wrapper (exit 0 = running,
	-- non-zero = stopped); report whether the TOML config is present so
	-- the page can distinguish "stopped" from "uninstalled".
	local code = sys.call("/etc/init.d/custos status >/dev/null 2>&1")
	local out = {
		status = (code == 0) and "running" or "stopped",
		config = nixio.fs.access("/etc/custos/custos.toml") and "yes" or "no",
	}

	-- The DB path is UCI-managed (the settings page); the default matches
	-- the init script so a fresh install with no UCI value still works.
	local uci = require("luci.model.uci").cursor()
	local db = uci:get("custos", "main", "db") or "/etc/custos/custos.db"
	out.db = db
	if not nixio.fs.access(db) then
		out.error = "数据库不存在: " .. sanitize(db, 200)
		luci.http.write_json(out)
		return
	end

	-- One CLI call per section; each prints a single JSON document (--json
	-- is part of the CLI contract). A missing binary or unreadable store
	-- degrades to an honest error field, never an empty-looking page.
	local function fetch_json(cmd)
		local raw = utl.exec(cmd)
		if raw == nil or raw == "" then
			return nil, "命令无输出（custos 已安装？）"
		end
		local parsed = luci.jsonc.parse(raw)
		if parsed == nil then
			return nil, "JSON 解析失败"
		end
		return parsed, nil
	end

	local data, err = fetch_json("/usr/bin/custos status --db " .. shell_quote(db) .. " --json 2>/dev/null")
	if err then
		out.error = "status: " .. err
	else
		out.counts = data.counts or {}
		out.health = {}
		for _, row in ipairs(data.health or {}) do
			out.health[#out.health + 1] = {
				resource_id = sanitize(row.resource_id, 64),
				probe = sanitize(row.probe, 32),
				succeeded = row.succeeded and true or false,
				observation = sanitize(row.observation, 120),
				at = ms_to_iso(row.at),
			}
		end
		out.audit = {}
		for _, row in ipairs(data.audit or {}) do
			out.audit[#out.audit + 1] = {
				id = sanitize(row.id, 16),
				at = ms_to_iso(row.at),
				severity = sanitize(row.severity, 16),
				event_type = sanitize(row.event_type, 48),
				resource_id = sanitize(row.resource_id, 64),
				run_id = sanitize(row.run_id, 32),
			}
		end
	end

	local runs, rerr = fetch_json("/usr/bin/custos runs --db " .. shell_quote(db) .. " --json 2>/dev/null")
	if rerr then
		out.error = out.error or ("runs: " .. rerr)
	else
		out.runs = {}
		for _, row in ipairs(runs.runs or {}) do
			out.runs[#out.runs + 1] = {
				id = sanitize(row.id, 32),
				resource_id = sanitize(row.resource_id, 64),
				status = sanitize(row.status, 24),
				trigger = sanitize(row.trigger, 64),
				updated_at = ms_to_iso(row.updated_at),
			}
		end
	end

	local incidents, ierr = fetch_json("/usr/bin/custos incidents --db " .. shell_quote(db) .. " --json 2>/dev/null")
	if ierr then
		out.error = out.error or ("incidents: " .. ierr)
	else
		out.incidents = {}
		for _, row in ipairs(incidents.incidents or {}) do
			out.incidents[#out.incidents + 1] = {
				id = sanitize(row.id, 16),
				resource_id = sanitize(row.resource_id, 64),
				fault_code = sanitize(row.fault_code, 32),
				summary = sanitize(row.summary, 120),
				opened_at = ms_to_iso(row.opened_at),
				closed = (row.closed_at ~= nil),
			}
		end
	end

	luci.http.write_json(out)
end
