-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

module("luci.controller.custos", package.seeall)

local tomledit = require "luci.custos.tomledit"

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

	entry({"admin", "services", "custos", "resources"},
		template("custos/resources"),
		_("监控资源"), 20).leaf = true

	entry({"admin", "services", "custos", "api", "overview"},
		call("api_overview")).leaf = true

	entry({"admin", "services", "custos", "api", "resources"},
		call("api_resources")).leaf = true

	entry({"admin", "services", "custos", "api", "write"},
		call("api_resource_write")).leaf = true
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

-- ---------------------------------------------------------------------------
-- Resource / recovery-policy editor (monitoring resources tab).
--
-- The TOML file is the authoritative configuration (route A of the design
-- note): LuCI edits it through the tomledit subset module, keeps unknown
-- lines verbatim, and gates every write on `custos validate` before the
-- service restarts. UCI stays responsible for runtime wiring only.
-- ---------------------------------------------------------------------------

local TOML_DEFAULT = "/etc/custos/custos.toml"

-- Types with a dedicated structured form; everything else goes through the
-- raw TOML textarea escape hatch (schema from crates/custos-config).
local SUPPORTED_TYPES = {
	["network-interface"] = true,
	["default-gateway"] = true,
	["custom-command"] = true,
	["icmp"] = true,
	["service"] = true,
}

local function toml_path()
	local uci = require("luci.model.uci").cursor()
	return uci:get("custos", "main", "conf") or TOML_DEFAULT
end

local function read_file(path)
	local f = io.open(path, "r")
	if not f then
		return nil
	end
	local text = f:read("*a")
	f:close()
	return text
end

-- Live health per resource id from the CLI (best effort; a missing binary
-- or store simply means every row shows "unknown").
local function health_map()
	local uci = require("luci.model.uci").cursor()
	local db = uci:get("custos", "main", "db") or "/etc/custos/custos.db"
	local raw = require("luci.util").exec(
		"/usr/bin/custos status --db " .. shell_quote(db) .. " --json 2>/dev/null")
	local map = {}
	local parsed = raw and raw ~= "" and luci.jsonc.parse(raw) or nil
	if parsed then
		for _, row in ipairs(parsed.health or {}) do
			map[row.resource_id] = row.succeeded and true or false
		end
	end
	return map
end

function api_resources()
	luci.http.prepare_content("application/json")
	local path = toml_path()
	local text = read_file(path)
	if not text then
		luci.http.write_json({ file = path,
			error = "无法读取配置文件: " .. sanitize(path, 200) })
		return
	end

	local doc = tomledit.parse(text)
	local health = health_map()
	local resources = {}
	for i, b in ipairs(doc.blocks) do
		local d = b.data
		local h = "unknown"
		if d.id ~= nil and health[d.id] ~= nil then
			h = health[d.id] and "healthy" or "failing"
		end
		local rp = d.recovery_policy or {}
		resources[#resources + 1] = {
			index = i - 1,
			id = d.id,
			type = d.type,
			display_name = d.display_name,
			enabled = d.enabled,
			depends_on = d.depends_on,
			supported = SUPPORTED_TYPES[d.type] and true or false,
			health = h,
			plan_count = rp.plans and #rp.plans or 0,
			allow_high_risk = rp.allow_high_risk == true,
			probe = d.probe,
			health_policy = d.health_policy,
			recovery_policy = rp,
			raw = table.concat(b.raw_lines),
		}
	end
	luci.http.write_json({ file = path, resources = resources })
end

-- Structural validation of one resource object coming from the form. The
-- engine validates semantics anyway (custos validate); this catches the
-- editor-level mistakes before touching the file.
local function validate_resource(data, doc, self_index)
	if type(data) ~= "table" then
		return "缺少资源数据"
	end
	local id = data.id
	if type(id) ~= "string" or id == "" or #id > 64
		or not id:match("^[%w%.%-]+$") then
		return "资源 id 只能包含字母、数字、点、横线（1-64 字符）"
	end
	for i, b in ipairs(doc.blocks) do
		if i ~= self_index and b.data.id == id then
			return "资源 id 已存在: " .. id
		end
	end
	if type(data.type) ~= "string" or data.type == "" then
		return "缺少资源类型"
	end
	local probe = data.probe or {}
	for _, k in ipairs({ "interval_ms", "timeout_ms" }) do
		local v = probe[k]
		if v ~= nil and (type(v) ~= "number" or v < 100) then
			return k .. " 必须是不小于 100 的整数毫秒"
		end
	end
	local plans = data.recovery_policy and data.recovery_policy.plans or {}
	local has_high = false
	for pi, plan in ipairs(plans) do
		local where = "恢复计划 #" .. pi
		if type(plan.plan_id) ~= "string" or plan.plan_id == "" then
			return where .. ": 缺少 plan_id"
		end
		if type(plan.applicable_faults) ~= "table"
			or #plan.applicable_faults == 0 then
			return where .. ": applicable_faults 不能为空"
		end
		if plan.risk ~= nil and plan.risk ~= "low"
			and plan.risk ~= "medium" and plan.risk ~= "high" then
			return where .. ": risk 只能是 low/medium/high"
		end
		if plan.risk == "high" then
			has_high = true
		end
		for _, k in ipairs({ "timeout_ms", "cooldown_ms" }) do
			local v = plan[k]
			if v ~= nil and (type(v) ~= "number" or v < 100) then
				return where .. ": " .. k .. " 必须是不小于 100 的整数毫秒"
			end
		end
		if plan.max_attempts ~= nil
			and (type(plan.max_attempts) ~= "number" or plan.max_attempts < 1) then
			return where .. ": max_attempts 必须不小于 1"
		end
		local action = plan.action or {}
		if type(action.program) ~= "string" or action.program == "" then
			return where .. ": 缺少 action.program"
		end
		if action.args ~= nil and type(action.args) ~= "table" then
			return where .. ": action.args 必须是字符串数组"
		end
	end
	if has_high and data.confirm_high_risk ~= true then
		return "包含 risk = high 的恢复计划，必须先勾选高危动作确认"
	end
	return nil
end

-- Backup → write temp file → custos validate → swap in → restart. The live
-- file is only ever replaced by a validated document; a failed validation
-- leaves it untouched (rollback) and surfaces the validator output, which
-- carries the offending line numbers.
local function apply_write(path, text)
	local bak = path .. ".bak"
	local tmp = path .. ".new"
	os.execute("cp -p " .. shell_quote(path) .. " " .. shell_quote(bak)
		.. " 2>/dev/null")
	local f = io.open(tmp, "w")
	if not f then
		return nil, "无法写入临时文件 " .. sanitize(tmp, 200)
	end
	f:write(text)
	f:close()
	local out = require("luci.util").exec(
		"/usr/bin/custos validate " .. shell_quote(tmp) .. " 2>&1; echo RC=$?")
	local rc = out:match("RC=(%d+)%s*$")
	if rc ~= "0" then
		os.execute("rm -f " .. shell_quote(tmp))
		return nil, out
	end
	if not os.execute("mv -f " .. shell_quote(tmp) .. " " .. shell_quote(path)) then
		os.execute("rm -f " .. shell_quote(tmp))
		return nil, "替换配置文件失败"
	end
	os.execute("/etc/init.d/custos restart >/dev/null 2>&1")
	return true
end

function api_resource_write()
	luci.http.prepare_content("application/json")
	luci.dispatcher.test_post_security()

	local payload = luci.http.formvalue("payload")
	local req = payload and payload ~= "" and luci.jsonc.parse(payload) or nil
	if type(req) ~= "table" or type(req.op) ~= "string" then
		luci.http.write_json({ ok = false, error = "请求格式错误" })
		return
	end

	local path = toml_path()
	local text = read_file(path)
	if not text then
		luci.http.write_json({ ok = false,
			error = "无法读取配置文件: " .. sanitize(path, 200) })
		return
	end
	local doc = tomledit.parse(text)

	local op = req.op
	local index = tonumber(req.index)
	local result, err

	if op == "delete" then
		if not index or not doc.blocks[index + 1] then
			result, err = nil, "资源序号无效"
		else
			table.remove(doc.blocks, index + 1)
			result, err = true, nil
		end
	elseif op == "raw" then
		if not index or not doc.blocks[index + 1] then
			result, err = nil, "资源序号无效"
		elseif type(req.raw) ~= "string" or req.raw == "" then
			result, err = nil, "缺少编辑文本"
		else
			result, err = tomledit.set_block_raw(doc, index + 1, req.raw)
		end
	elseif op == "save" or op == "create" then
		local data = req.data
		local self_index = (op == "save") and (index and index + 1) or nil
		if op == "save" and (not self_index or not doc.blocks[self_index]) then
			result, err = nil, "资源序号无效"
		else
			-- The confirm checkbox travels at the request level; hand it to
			-- the validator inside the data it inspects.
			data.confirm_high_risk = (req.confirm_high_risk == true)
			err = validate_resource(data, doc, self_index)
			if err then
				result = nil
			else
				data.confirm_high_risk = nil
				-- The engine requires an explicit opt-in on the resource next
				-- to any high-risk plan; the confirm checkbox doubles as it.
				for _, plan in ipairs((data.recovery_policy and data.recovery_policy.plans) or {}) do
					if plan.risk == "high" then
						data.recovery_policy.allow_high_risk = true
						break
					end
				end
				if op == "save" then
					result = tomledit.replace_block(doc, self_index, data)
				else
					result = tomledit.append_block(doc, data, {})
				end
			end
		end
	else
		result, err = nil, "未知操作: " .. sanitize(op, 32)
	end

	if not result then
		luci.http.write_json({ ok = false, error = err or "操作失败" })
		return
	end

	result, err = apply_write(path, tomledit.render(doc))
	if not result then
		luci.http.write_json({ ok = false, error = "写入校验未通过，配置已回滚",
			output = err })
	else
		luci.http.write_json({ ok = true, message = "已保存，custos validate 通过，服务已重启" })
	end
end
