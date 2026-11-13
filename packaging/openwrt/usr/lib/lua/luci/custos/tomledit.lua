-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Minimal TOML subset reader/renderer for the custos LuCI resource editor.
--
-- Scope (deliberately narrow): [[resources]] blocks with dotted
-- [resources.*] subtables and key = value lines whose values are basic or
-- literal strings, numbers, booleans, or inline arrays of those. Anything
-- outside the subset stays verbatim: untouched blocks round-trip
-- byte-identical (raw lines are kept, not re-rendered), and unrecognized
-- lines inside an edited block are re-emitted at the block tail so exotic
-- configuration never silently disappears. Well-formedness is enforced by
-- `custos validate` before any write is accepted; this module only has to
-- preserve and render.
--
-- Written for Lua 5.1 (what OpenWrt ships); parses on 5.4 as well.

local M = {}

-- Recognized keys in the order render_resource emits them (mirrors the
-- RawProbe union in crates/custos-config/src/schema.rs).
local DIRECT_ORDER = { "id", "type", "display_name", "enabled", "depends_on" }
local PROBE_ORDER = {
	"command", "pid_file", "interval_ms", "timeout_ms", "max_output_bytes",
	"url", "fail_if_matches", "contains", "allowed_statuses",
	"max_redirects", "max_response_bytes", "response_time_threshold_ms",
	"follow_redirects", "verify_tls", "host", "port", "tls", "server_name",
	"warn_days", "crit_days", "fallback_port", "tcp_fallback", "kind",
	"query", "user", "password", "database", "name", "server",
	"record_type", "expect", "proxy", "target", "username", "unit",
	"mount_point", "warn_pct", "crit_pct", "check_swap", "warn_load",
	"crit_load", "path", "pattern", "max_bytes_per_check", "container",
	"max_restarts", "interface", "connect_port",
}
local HP_ORDER = { "failure_threshold", "success_threshold" }
local PLAN_ORDER = { "plan_id", "applicable_faults", "risk", "idempotent",
	"timeout_ms", "max_attempts", "cooldown_ms" }

local PROBE_SET = {}
for _, k in ipairs(PROBE_ORDER) do PROBE_SET[k] = true end
local DIRECT_SET = {}
for _, k in ipairs(DIRECT_ORDER) do DIRECT_SET[k] = true end
local HP_SET = {}
for _, k in ipairs(HP_ORDER) do HP_SET[k] = true end
local PLAN_SET = {}
for _, k in ipairs(PLAN_ORDER) do PLAN_SET[k] = true end

-- Split into lines that keep their own trailing "\n", so joining is exact.
local function split_lines(text)
	local lines = {}
	local pos = 1
	while pos <= #text do
		local nl = text:find("\n", pos, true)
		if not nl then
			lines[#lines + 1] = text:sub(pos)
			break
		end
		lines[#lines + 1] = text:sub(pos, nl)
		pos = nl + 1
	end
	return lines
end
M.split_lines = split_lines

local function trim(s)
	return (s:gsub("^[ \t\r\n]+", ""):gsub("[ \t\r\n]+$", ""))
end

-- Scan a quoted string starting at s:sub(i, i) (either quote flavor).
-- Returns the decoded value and the index after the closing quote.
local function scan_string(s, i)
	local q = s:sub(i, i)
	local out = {}
	i = i + 1
	while i <= #s do
		local c = s:sub(i, i)
		if q == "'" then
			if c == "'" then
				return table.concat(out), i + 1
			end
			out[#out + 1] = c
			i = i + 1
		else
			if c == "\\" then
				local n = s:sub(i + 1, i + 1)
				local map = { n = "\n", t = "\t", r = "\r",
					['"'] = '"', ["\\"] = "\\", b = "\b", f = "\f" }
				out[#out + 1] = map[n] or n
				i = i + 2
			elseif c == '"' then
				return table.concat(out), i + 1
			else
				out[#out + 1] = c
				i = i + 1
			end
		end
	end
	return nil, nil -- unterminated
end

-- Scan an inline array [ ... ] of strings/numbers starting at s:sub(i, i).
local function scan_array(s, i)
	local arr = {}
	i = i + 1
	while true do
		local _, j = s:find("^[ \t]*", i)
		i = j + 1
		if i > #s then
			return nil, nil -- unterminated (e.g. multiline arrays)
		end
		local c = s:sub(i, i)
		if c == "]" then
			return arr, i + 1
		elseif c == "," then
			i = i + 1
		elseif c == '"' or c == "'" then
			local v, ni = scan_string(s, i)
			if not v then return nil, nil end
			arr[#arr + 1] = v
			i = ni
		else
			local tok = s:match("^[%w_%-%.%+]+", i)
			local num = tok and tonumber(tok) or nil
			if not num then return nil, nil end
			arr[#arr + 1] = num
			i = i + #tok
		end
	end
end

-- Parse the value part of a `key = value` line (rest of line after "=").
-- Returns value, or nil plus a reason. Trailing whitespace/comment is fine;
-- anything else after the value marks the line as unsupported.
local function parse_value(s)
	local _, j = s:find("^[ \t]*", 1)
	local i = j + 1
	if i > #s then
		return nil, "empty value"
	end
	local c = s:sub(i, i)
	local v, ni
	if c == '"' or c == "'" then
		v, ni = scan_string(s, i)
		if not v then return nil, "unterminated string" end
	elseif c == "[" then
		v, ni = scan_array(s, i)
		if not v then return nil, "unterminated array" end
	else
		local tok = s:match("^[%w_%-%.%+]+", i)
		if not tok then
			return nil, "unsupported value"
		end
		if tok == "true" or tok == "false" then
			v, ni = (tok == "true"), i + #tok
		else
			local num = tonumber(tok)
			if not num then return nil, "unsupported value: " .. tok end
			v, ni = num, i + #tok
		end
	end
	local rest = s:sub(ni)
	if not (rest:match("^[ \t]*$") or rest:match("^[ \t]*#")) then
		return nil, "trailing content after value"
	end
	return v
end

local function toml_escape(s)
	return '"' .. tostring(s)
		:gsub("\\", "\\\\")
		:gsub('"', '\\"')
		:gsub("\n", "\\n")
		:gsub("\t", "\\t")
		:gsub("\r", "\\r")
		.. '"'
end

local function render_value(v)
	local tv = type(v)
	if tv == "string" then
		return toml_escape(v)
	elseif tv == "boolean" or tv == "number" then
		return tostring(v)
	elseif tv == "table" then
		local parts = {}
		for _, item in ipairs(v) do
			parts[#parts + 1] = render_value(item)
		end
		return "[" .. table.concat(parts, ", ") .. "]"
	end
	return tostring(v)
end

-- Route one recognized key = value into the block structure. Returns false
-- when the key/section is not part of the subset (caller keeps the raw line).
local function assign(cur, section, plan, key, val)
	local d = cur.data
	if section == "direct" and DIRECT_SET[key] then
		d[key] = val
		return true
	elseif section == "probe" and PROBE_SET[key] then
		d.probe[key] = val
		return true
	elseif section == "health_policy" and HP_SET[key] then
		d.health_policy = d.health_policy or {}
		d.health_policy[key] = val
		return true
	elseif section == "recovery"
		and (key == "max_attempts" or key == "allow_high_risk") then
		d.recovery_policy = d.recovery_policy or { plans = {} }
		d.recovery_policy[key] = val
		return true
	elseif section == "plan" and plan and PLAN_SET[key] then
		plan[key] = val
		return true
	elseif section == "plan_action" and plan
		and (key == "program" or key == "args") then
		plan.action[key] = val
		return true
	elseif section == "plan_verification" and plan and key == "status_is" then
		plan.verification[key] = val
		return true
	end
	return false
end

-- Parse the whole document. Result layout:
--   head   = verbatim lines before the first [[resources]]
--   blocks = { { pre_lines, raw_lines, data, extra }, ... }
--   tail   = verbatim lines after the last block ends
-- Comment/blank lines at the end of one resource or between blocks attach
-- forward to the next block (pre_lines), so deleting a resource takes its
-- leading comment with it while `version = 1` and the following resources'
-- comments stay put. Comment lines followed by more body lines inside one
-- block stay verbatim in that block.
function M.parse(text)
	local doc = { head = {}, blocks = {}, tail = {} }
	local pending = {}
	local inner = {} -- comment/blank lines buffered inside the current block
	local cur, section, cur_plan = nil, nil, nil

	local function flush_content(line)
		local sink = (#doc.blocks == 0) and doc.head or doc.tail
		for _, pl in ipairs(pending) do sink[#sink + 1] = pl end
		pending = {}
		sink[#sink + 1] = line
	end

	for _, line in ipairs(split_lines(text)) do
		local t = trim(line)
		local path, is_arr
		local hp = t:match("^%[%[(.-)%]%]$")
		if hp then
			is_arr, path = true, hp
		else
			local tp = t:match("^%[(.-)%]$")
			if tp then
				is_arr, path = false, tp
			end
		end

		if is_arr == nil then
			-- plain line
			if cur then
				if t == "" or t:sub(1, 1) == "#" then
					inner[#inner + 1] = line
				else
					-- a body line ends any buffered comment run: those stay
					-- verbatim inside this block
					for _, pl in ipairs(inner) do
						cur.raw_lines[#cur.raw_lines + 1] = pl
					end
					inner = {}
					cur.raw_lines[#cur.raw_lines + 1] = line
					local key, rest = t:match("^([%w_%-]+)[ \t]*=[ \t]*(.+)$")
					local val
					if key then
						val = parse_value(rest)
					end
					if val ~= nil and assign(cur, section, cur_plan, key, val) then
						-- recorded into the structured data
					else
						cur.extra[#cur.extra + 1] = line
					end
				end
			elseif t == "" or t:sub(1, 1) == "#" then
				pending[#pending + 1] = line
			else
				flush_content(line)
			end
		else
			-- header line: decide where the buffered comment run goes. A
			-- subtable header ([resources.*]) keeps it inside the block as
			-- body; a new block or global header attaches it forward.
			local is_block_header = is_arr and path == "resources"
			local in_block_header = cur and not is_block_header and path
				and (path == "resources" or path:match("^resources%."))
			if in_block_header then
				for _, pl in ipairs(inner) do
					cur.raw_lines[#cur.raw_lines + 1] = pl
				end
			else
				for _, pl in ipairs(inner) do pending[#pending + 1] = pl end
			end
			inner = {}
			if is_block_header then
				cur = { pre_lines = pending, raw_lines = { line },
					data = { probe = {} }, extra = {} }
				pending = {}
				section, cur_plan = "direct", nil
				doc.blocks[#doc.blocks + 1] = cur
			elseif cur and is_arr and path == "resources.recovery_policy.plans" then
				cur.raw_lines[#cur.raw_lines + 1] = line
				cur.data.recovery_policy = cur.data.recovery_policy or {}
				cur.data.recovery_policy.plans = cur.data.recovery_policy.plans or {}
				cur_plan = { action = {}, verification = {} }
				cur.data.recovery_policy.plans[#cur.data.recovery_policy.plans + 1] = cur_plan
				section = "plan"
			elseif cur and path and (path == "resources" or path:match("^resources%.")) then
				-- subtable header within the current resource; the plan-scoped
				-- sections keep the active plan, the rest drop it
				cur.raw_lines[#cur.raw_lines + 1] = line
				if not is_arr and path == "resources.recovery_policy.plans.action" then
					section = "plan_action"
				elseif not is_arr and path == "resources.recovery_policy.plans.verification" then
					section = "plan_verification"
				elseif not is_arr and path == "resources.probe" then
					section, cur_plan = "probe", nil
				elseif not is_arr and path == "resources.health_policy" then
					section, cur_plan = "health_policy", nil
				elseif not is_arr and path == "resources.recovery_policy" then
					section, cur_plan = "recovery", nil
				else
					section, cur_plan = nil, nil
				end
			else
				if cur then
					-- a global header closes the resource section
					cur = nil
					section, cur_plan = nil, nil
				end
				flush_content(line)
			end
		end
	end
	for _, pl in ipairs(pending) do doc.tail[#doc.tail + 1] = pl end
	for _, pl in ipairs(inner) do doc.tail[#doc.tail + 1] = pl end
	return doc
end

-- Join the document back to text. Untouched blocks contribute their raw
-- lines verbatim (the round-trip contract); edited blocks have had their
-- raw_lines replaced by render_resource output.
function M.render(doc)
	local parts = {}
	local function add(lines)
		for _, l in ipairs(lines) do parts[#parts + 1] = l end
	end
	add(doc.head)
	for _, b in ipairs(doc.blocks) do
		add(b.pre_lines)
		add(b.raw_lines)
	end
	add(doc.tail)
	return table.concat(parts)
end

-- Render one resource's structured data (plus verbatim extra lines) to the
-- canonical block body, starting with the [[resources]] header.
function M.render_resource(data, extra)
	local out = {}
	local function emit(line)
		out[#out + 1] = line .. "\n"
	end

	emit("[[resources]]")
	for _, k in ipairs(DIRECT_ORDER) do
		local v = data[k]
		if k == "enabled" then
			if type(v) == "boolean" then
				emit("enabled = " .. tostring(v))
			end
		elseif v ~= nil and (k ~= "depends_on" or (type(v) == "table" and #v > 0)) then
			emit(k .. " = " .. render_value(v))
		end
	end

	local probe_nonnil = 0
	for _, k in ipairs(PROBE_ORDER) do
		if data.probe and data.probe[k] ~= nil then
			probe_nonnil = probe_nonnil + 1
		end
	end
	if probe_nonnil > 0 then
		emit("")
		emit("[resources.probe]")
		for _, k in ipairs(PROBE_ORDER) do
			local v = data.probe[k]
			if v ~= nil then
				emit(k .. " = " .. render_value(v))
			end
		end
	end

	local hp = data.health_policy
	if hp and (hp.failure_threshold ~= nil or hp.success_threshold ~= nil) then
		emit("")
		emit("[resources.health_policy]")
		for _, k in ipairs(HP_ORDER) do
			if hp[k] ~= nil then
				emit(k .. " = " .. render_value(hp[k]))
			end
		end
	end

	local rp = data.recovery_policy
	local plans = rp and rp.plans or nil
	local has_plans = plans and #plans > 0
	if rp and (rp.allow_high_risk == true or rp.max_attempts ~= nil or has_plans) then
		if rp.allow_high_risk == true or rp.max_attempts ~= nil then
			emit("")
			emit("[resources.recovery_policy]")
			if rp.allow_high_risk == true then
				emit("allow_high_risk = true")
			end
			if rp.max_attempts ~= nil then
				emit("max_attempts = " .. render_value(rp.max_attempts))
			end
		end
		if has_plans then
			for _, plan in ipairs(plans) do
				emit("")
				emit("[[resources.recovery_policy.plans]]")
				for _, k in ipairs(PLAN_ORDER) do
					local v = plan[k]
					if k == "idempotent" then
						if type(v) == "boolean" then
							emit("idempotent = " .. tostring(v))
						end
					elseif v ~= nil then
						emit(k .. " = " .. render_value(v))
					end
				end
				if plan.action and (plan.action.program ~= nil or plan.action.args ~= nil) then
					emit("")
					emit("[resources.recovery_policy.plans.action]")
					if plan.action.program ~= nil then
						emit("program = " .. render_value(plan.action.program))
					end
					if plan.action.args ~= nil then
						emit("args = " .. render_value(plan.action.args))
					end
				end
				if plan.verification and plan.verification.status_is ~= nil then
					emit("")
					emit("[resources.recovery_policy.plans.verification]")
					emit("status_is = " .. render_value(plan.verification.status_is))
				end
			end
		end
	end

	for _, l in ipairs(extra or {}) do
		if l:sub(-1) == "\n" then
			out[#out + 1] = l
		else
			out[#out + 1] = l .. "\n"
		end
	end
	return table.concat(out)
end

-- Replace block i's body with a structured re-render, keeping its comment
-- preamble and unrecognized lines intact.
function M.replace_block(doc, i, data, extra)
	local b = doc.blocks[i]
	if not b then return nil, "no such resource" end
	b.data = data
	b.extra = extra or b.extra
	b.raw_lines = split_lines(M.render_resource(data, b.extra))
	return b
end

-- Append a new resource block at the end of the resources section.
function M.append_block(doc, data, extra)
	local block = {
		pre_lines = {},
		raw_lines = split_lines(M.render_resource(data, extra)),
		data = data,
		extra = extra or {},
	}
	doc.blocks[#doc.blocks + 1] = block
	return block
end

-- Swap block i's body for raw TOML text (advanced textarea mode) and
-- re-parse its structured view from that text. Rejects fragments without
-- exactly one [[resources]] block; value-level errors are left to
-- `custos validate` (which reports the line numbers).
function M.set_block_raw(doc, i, rawtext)
	local b = doc.blocks[i]
	if not b then return nil, "no such resource" end
	local frag = M.parse(rawtext)
	if #frag.blocks ~= 1 then
		return nil, "编辑文本必须包含且只包含一个 [[resources]] 块"
	end
	if rawtext:sub(-1) ~= "\n" then
		rawtext = rawtext .. "\n"
	end
	b.raw_lines = split_lines(rawtext)
	b.data = frag.blocks[1].data
	b.extra = frag.blocks[1].extra
	return b
end

return M
