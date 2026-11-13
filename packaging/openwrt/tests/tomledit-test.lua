-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Unit tests for the LuCI TOML subset editor (tomledit.lua).
-- Run on host or device:  lua packaging/openwrt/tests/tomledit-test.lua
-- The fixture is the verbatim custos.toml of the acceptance device
-- (192.168.5.11); the round-trip gate requires byte identity on it.

local function script_dir()
	local src = debug.getinfo(1, "S").source:sub(2)
	return src:match("^(.*)/[^/]+$") or "."
end

local base = script_dir() .. "/.."
package.path = base .. "/usr/lib/lua/luci/custos/?.lua;" .. package.path

local te = require "tomledit"

local passed = 0
local function ok(cond, name)
	if not cond then
		io.stderr:write("FAIL: " .. name .. "\n")
		os.exit(1)
	end
	print("ok " .. passed + 1 .. " - " .. name)
	passed = passed + 1
end

local function read(path)
	local f = io.open(path, "r")
	local t = f:read("*a")
	f:close()
	return t
end

local fixture = read(script_dir() .. "/fixture-custos.toml")

-- 1. Round-trip: parse + render with no modification is byte-identical.
local doc = te.parse(fixture)
ok(#doc.blocks == 4, "fixture has 4 resource blocks")
ok(te.render(doc) == fixture, "round-trip on real device toml is byte-identical")

-- 2. Structured fields parsed correctly.
ok(doc.blocks[1].data.id == "wan", "block 1 id")
ok(doc.blocks[1].data.type == "network-interface", "block 1 type")
ok(doc.blocks[1].data.probe.interface == "eth0", "block 1 probe.interface")
ok(doc.blocks[3].data.id == "dnsmasq", "block 3 id")
ok(doc.blocks[3].data.probe.command[1] == "sh"
	and doc.blocks[3].data.probe.command[3] ==
	"pidof dnsmasq | grep -q . && echo OK || exit 1",
	"block 3 probe.command array parsed")
ok(doc.blocks[4].data.probe.host == "192.168.5.1", "block 4 probe.host")
ok(doc.head[1] == "version = 1\n", "head keeps version line")

-- 3. Editing one block leaves every other byte in place: edit the gateway
-- probe interval, render, then diff by reconstructing the untouched parts.
local doc2 = te.parse(fixture)
local b2 = doc2.blocks[2]
ok(b2.data.id == "gateway", "block 2 is gateway")
b2.data.probe.interval_ms = 7000
b2.raw_lines = te.split_lines(te.render_resource(b2.data, b2.extra))
local out2 = te.render(doc2)
ok(out2:find("interval_ms = 7000") ~= nil, "edited interval present")
ok(out2:find('id = "wan"') ~= nil
	and out2:find('interface = "eth0"') ~= nil,
	"other resources kept")
-- Untouched blocks byte-identical: everything except block 2's body.
local plain = {}
for line in (fixture .. " "):gmatch("(.-)\n") do
	plain[#plain + 1] = line .. "\n"
end
ok(out2:find("interval_ms = 10000\n\n# 默认网关") ~= nil,
	"wan to gateway transition intact")
ok(out2:find("interval_ms = 7000\n\n# dnsmasq") ~= nil,
	"edited gateway flows into dnsmasq block intact")
ok(out2:find('id = "gateway"\ntype = "default%-gateway"\n\n%[resources%.probe%]\ninterval_ms = 7000\n') ~= nil,
	"edited gateway block matches canonical layout")

-- 4. version = 1 survives deleting the first block.
local doc3 = te.parse(fixture)
table.remove(doc3.blocks, 1)
local out3 = te.render(doc3)
ok(out3:find("version = 1") == 1, "version line stays at top after delete")
ok(out3:find("id = \"wan\"") == nil, "deleted resource gone")
ok(out3:find("id = \"gateway\"") ~= nil, "remaining resource kept")
ok(out3:find("# OpenWrt 自身") == nil, "deleted block's comment went with it")
ok(out3:find("# 默认网关") ~= nil, "kept block keeps its own comment")

-- 5. Append a resource: structured render, then re-parse gives same fields.
local doc4 = te.parse(fixture)
te.append_block(doc4, {
	id = "clock",
	type = "custom-command",
	probe = { command = { "sh", "-c", "exit 0" }, interval_ms = 5000 },
	health_policy = { failure_threshold = 2, success_threshold = 2 },
	recovery_policy = { plans = { {
		plan_id = "noop",
		applicable_faults = { "process_exited" },
		risk = "low",
		idempotent = false,
		timeout_ms = 5000,
		max_attempts = 2,
		cooldown_ms = 30000,
		action = { program = "sh", args = { "-c", "echo done" } },
		verification = { status_is = "healthy" },
	} } },
}, {})
local out4 = te.render(doc4)
local doc5 = te.parse(out4)
ok(#doc5.blocks == 5, "appended block present")
local clock = doc5.blocks[5].data
ok(clock.id == "clock" and clock.type == "custom-command",
	"appended fields round-trip")
ok(clock.probe.command[2] == "-c", "appended command array round-trip")
ok(clock.health_policy.failure_threshold == 2, "health_policy round-trip")
local plan = clock.recovery_policy.plans[1]
ok(plan.applicable_faults[1] == "process_exited"
	and plan.risk == "low" and plan.idempotent == false
	and plan.max_attempts == 2 and plan.cooldown_ms == 30000
	and plan.action.program == "sh" and plan.action.args[1] == "-c"
	and plan.verification.status_is == "healthy",
	"plan fields round-trip")
ok(doc5.blocks[1].raw_lines[1] == "[[resources]]\n",
	"earlier blocks untouched by append")

-- 6. String escaping: quotes and backslashes survive render -> parse.
local doc6 = te.parse(fixture)
te.append_block(doc6, {
	id = "esc",
	type = "custom-command",
	probe = { command = { "sh", "-c", "echo \"a\\b\" > /tmp/x" } },
}, {})
local rt = te.parse(te.render(doc6)).blocks[5].data
ok(rt.probe.command[3] == "echo \"a\\b\" > /tmp/x",
	"quotes and backslashes escape correctly")

-- 7. Unrecognized structure is preserved verbatim on structured edit.
local exotic = [==[
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
display_name = "cloudflare tunnel"
dependencies = [
  { resource = "network", requirement = "conditional", conditional_on = "proxy_mode", condition = { status_is = "healthy" } },
]

[resources.probe]
command = ["sh", "-c", "exit 0"]
]==]
local doc7 = te.parse(exotic)
ok(#doc7.blocks == 1, "exotic file parses")
ok(doc7.blocks[1].data.id == "tunnel", "exotic direct keys parsed")
ok(#doc7.blocks[1].extra == 3, "multiline array kept as 3 verbatim lines")
doc7.blocks[1].data.display_name = "renamed"
doc7.blocks[1].raw_lines = te.split_lines(
	te.render_resource(doc7.blocks[1].data, doc7.blocks[1].extra))
local out7 = te.render(doc7)
ok(out7:find('dependencies = %[\n  { resource = "network"') ~= nil,
	"multiline dependencies array re-emitted verbatim")
ok(out7:find('display_name = "renamed"') ~= nil, "structured edit applied")

-- 8. set_block_raw swaps the body and reparses the structured view.
local doc8 = te.parse(fixture)
local newraw = '[[resources]]\nid = "gateway"\ntype = "default-gateway"\n\n[resources.probe]\ninterval_ms = 9000\n'
local b, err = te.set_block_raw(doc8, 2, newraw)
ok(b ~= nil, "set_block_raw succeeds (" .. tostring(err) .. ")")
ok(doc8.blocks[2].data.probe.interval_ms == 9000,
	"raw edit reparses structured view")
ok(te.render(doc8):find("interval_ms = 9000") ~= nil, "raw edit rendered")

-- 9. set_block_raw rejects fragments without exactly one block.
local doc9 = te.parse(fixture)
local bad, berr = te.set_block_raw(doc9, 1, 'id = "wan"\n')
ok(bad == nil and berr ~= nil, "fragment without header rejected")
local multi, merr = te.set_block_raw(doc9, 1,
	'[[resources]]\nid = "a"\ntype = "exec"\n\n[[resources]]\nid = "b"\ntype = "exec"\n')
ok(multi == nil and merr ~= nil, "fragment with two blocks rejected")

-- 10. Unknown keys inside known sections stay in extra and survive edits.
local with_extra = [==[
[[resources]]
id = "odd"
type = "exec"

[resources.probe]
command = ["true"]
interval_ms = 1000
custom_field = { a = 1 }
]==]
local doc10 = te.parse(with_extra)
ok(doc10.blocks[1].extra[1]:find("custom_field") ~= nil,
	"inline-table value kept verbatim")
doc10.blocks[1].data.probe.interval_ms = 2000
doc10.blocks[1].raw_lines = te.split_lines(
	te.render_resource(doc10.blocks[1].data, doc10.blocks[1].extra))
local out10 = te.render(doc10)
ok(out10:find("custom_field = { a = 1 }") ~= nil,
	"unknown key survives structured edit")
ok(out10:find("interval_ms = 2000") ~= nil, "edit applied around it")

-- 11. No trailing newline in the file is preserved.
local nonl = fixture:gsub("\n$", "")
local doc11 = te.parse(nonl)
ok(te.render(doc11) == nonl, "missing trailing newline round-trips")

-- 12. Global section after resources goes to tail and stays put on edit.
local withtail = fixture .. "\n[permissions]\ndefault_allow_high_risk = false\n"
local doc12 = te.parse(withtail)
doc12.blocks[4].data.probe.timeout_ms = 2500
doc12.blocks[4].raw_lines = te.split_lines(
	te.render_resource(doc12.blocks[4].data, doc12.blocks[4].extra))
local out12 = te.render(doc12)
ok(out12:find("timeout_ms = 2500") ~= nil, "tail-mode edit applied")
ok(out12:find("%[permissions%]\ndefault_allow_high_risk = false\n") ~= nil,
	"global section after resources intact")
ok(out12:sub(-1) == "\n", "document still newline-terminated")

print("\nall " .. passed .. " tests passed")
