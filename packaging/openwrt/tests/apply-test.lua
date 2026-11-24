-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Unit tests for the TOML write chain (luci.custos.apply, C-SET-UX-04/05):
-- the six-step order, the failure invariant (live keeps the last good
-- config, .bak only updated just before a validated replacement), the md5
-- concurrency guard and the four-state restart feedback. All side effects
-- are fakes; nothing here touches the filesystem or spawns a process.
-- Run:  lua packaging/openwrt/tests/apply-test.lua

local function script_dir()
	local src = debug.getinfo(1, "S").source:sub(2)
	return src:match("^(.*)/[^/]+$") or "."
end

local base = script_dir() .. "/.."
package.path = base .. "/usr/lib/lua/luci/custos/?.lua;" .. package.path

local apply = require "apply"

local passed = 0
local function ok(cond, name)
	if not cond then
		io.stderr:write("FAIL: " .. name .. "\n")
		os.exit(1)
	end
	print("ok " .. passed + 1 .. " - " .. name)
	passed = passed + 1
end

local function eq(a, b, name)
	ok(a == b, name .. " (" .. tostring(a) .. " ~= " .. tostring(b) .. ")")
end

local P = "/etc/custos/custos.toml"
local T = P .. ".new"
local B = P .. ".bak"

-- A fake world: a path->content map plus call log. md5 is derived from the
-- content so copies verify naturally; md5_map overrides single paths (used
-- to simulate a corrupted backup whose hash does not match).
local function mk(t)
	t = t or {}
	local calls = {}
	local files = { [P] = t.live }
	if t.bak ~= nil then files[B] = t.bak end
	local polled = 0
	local fns = {
		read = function(p) return files[p] end,
		md5 = function(p)
			if t.md5_map and t.md5_map[p] ~= nil then return t.md5_map[p] end
			if files[p] == nil then return nil end
			return "h" .. #tostring(files[p]) .. ":" .. tostring(files[p]):sub(1, 8)
		end,
		write = function(p, text)
			table.insert(calls, "write " .. p)
			if t.write_ok == false then return false end
			files[p] = text
			return true
		end,
		copy = function(src, dst)
			table.insert(calls, "copy " .. src .. " " .. dst)
			if t.copy_ok == false then return false end
			if files[src] == nil then return false end
			files[dst] = files[src]
			return true
		end,
		move = function(src, dst)
			table.insert(calls, "move " .. src .. " " .. dst)
			if t.move_ok == false then return false end
			if files[src] == nil then return false end
			files[dst] = files[src]
			files[src] = nil
			return true
		end,
		rm = function(p)
			table.insert(calls, "rm " .. p)
			files[p] = nil
		end,
		validate = function(p)
			table.insert(calls, "validate " .. p)
			return t.validate_rc or 0, t.validate_out or ""
		end,
		restart = function()
			table.insert(calls, "restart")
			return t.restart_rc or 0
		end,
		running = function()
			polled = polled + 1
			table.insert(calls, "poll")
			if t.run_seq then
				local v = t.run_seq[polled]
				if v ~= nil then
					return v
				end
			end
			if t.run_after_polls and polled >= t.run_after_polls then
				return true
			end
			return t.running == true
		end,
		sleep = function() end,
		enabled = function() return t.enabled ~= false end,
	}
	local function joined(sep)
		return table.concat(calls, sep)
	end
	return fns, files, calls, joined
end

local function find(calls, what)
	for i, c in ipairs(calls) do
		if c:sub(1, #what) == what then
			return i
		end
	end
	return nil
end

-- ---- write chain: success path -----------------------------------------

do
	local fns, files, calls = mk({ live = "old", running = true })
	local state, info = apply.write({ path = P, candidate = "new",
		base_md5 = nil, fns = fns })
	eq(state, "confirmed", "happy path settles to confirmed")
	eq(files[P], "new", "happy path replaces live with the candidate")
	eq(files[B], "old", "happy path keeps the previous config as .bak")
	eq(files[T], nil, "happy path leaves no temp file behind")
	eq(info.backup, B, "happy path reports the backup path")
	local iv, ic, im = find(calls, "validate"), find(calls, "copy"), find(calls, "move")
	ok(iv and ic and im and iv < ic and ic < im,
		"order is validate -> backup copy -> atomic move")
	eq(find(calls, "restart") ~= nil, true, "happy path restarts the service")
end

do
	-- os.execute on Lua 5.2+ returns booleans; the chain must accept them.
	local fns = mk({ live = "old", running = true })
	fns.restart = function() return true end
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "confirmed", "boolean restart rc (Lua 5.2+) counts as success")
end

do
	local fns, files = mk({ live = "old", run_after_polls = 3 })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "confirmed", "late start within the window still confirms")
	eq(files[P], "new", "late start keeps the new config")
end

do
	-- Acceptance §4 "启动后退出": the service comes up, is seen running
	-- once, then dies. A single running observation must not confirm.
	local fns = mk({ live = "old", run_seq = { true, false } })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "restart_failed", "start-then-exit is restart_failed")
end

do
	-- Flap then settle: up, gone, up and stable -> confirmed.
	local fns = mk({ live = "old", run_seq = { true, false, true, true } })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "confirmed", "flap that settles running still confirms")
end

-- ---- four states: restart_failed / stopped_by_user ----------------------

do
	local fns, files, calls = mk({ live = "old", restart_rc = 1 })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "restart_failed", "restart rc /= 0 is restart_failed")
	eq(files[P], "new", "restart failure keeps the validated new config")
	eq(files[B], "old", "restart failure keeps the backup for restore")
	ok(find(calls, "poll") == nil, "failed restart skips the polling window")
end

do
	local fns = mk({ live = "old", enabled = false })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "stopped_by_user", "timeout with UCI disabled is stopped_by_user")
end

do
	local fns = mk({ live = "old", enabled = true })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "restart_failed", "timeout with UCI enabled is restart_failed")
end

-- ---- no-op and concurrency guard ----------------------------------------

do
	local fns, files, calls = mk({ live = "same" })
	local state = apply.write({ path = P, candidate = "same", fns = fns })
	eq(state, "noop", "byte-identical candidate is a no-op")
	eq(find(calls, "validate"), nil, "no-op runs no validation")
	eq(find(calls, "copy"), nil, "no-op makes no backup")
	eq(find(calls, "restart"), nil, "no-op restarts nothing")
	eq(files[P], "same", "no-op leaves live untouched")
end

do
	local fns, files, calls = mk({ live = "current" })
	local state = apply.write({ path = P, candidate = "new",
		base_md5 = "stale-baseline", fns = fns })
	eq(state, "stale", "md5 mismatch is rejected as stale")
	eq(files[P], "current", "stale rejection leaves live untouched")
	eq(find(calls, "validate"), nil, "stale rejection runs nothing")
end

do
	local fns, files = mk({ live = "current", running = true })
	local state = apply.write({ path = P, candidate = "new",
		base_md5 = "h7:current", fns = fns })
	eq(state, "confirmed", "matching baseline md5 passes the guard")
	eq(files[P], "new", "matching baseline lets the write through")
end

-- ---- failure invariant: validation failure has zero side effects --------

do
	local fns, files, calls = mk({ live = "good", bak = "older",
		validate_rc = 1, validate_out = "custos.toml.new:5: unknown key" })
	local state, info = apply.write({ path = P, candidate = "broken", fns = fns })
	eq(state, "invalid", "failed validation is invalid")
	eq(files[P], "good", "validation failure keeps live byte-identical")
	eq(files[B], "older", "validation failure never touches .bak")
	eq(files[T], nil, "validation failure removes the temp file")
	eq(find(calls, "copy"), nil, "validation failure makes no backup")
	eq(find(calls, "restart"), nil, "validation failure restarts nothing")
	eq(info.output, "custos.toml.new:5: unknown key",
		"validation failure surfaces the validator output")
end

-- ---- verified backup ------------------------------------------------------

do
	local fns, files = mk({ live = "good", copy_ok = false })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "backup_failed", "failed backup copy aborts")
	eq(files[P], "good", "failed backup copy leaves live untouched")
	eq(files[T], nil, "failed backup copy removes the temp file")
end

do
	-- The copy succeeded but its hash does not match the live file: the
	-- backup cannot be trusted, so the chain must abort.
	local fns, files = mk({ live = "good", md5_map = { [B] = "corrupted" } })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "backup_failed", "backup hash mismatch aborts")
	eq(files[P], "good", "backup hash mismatch leaves live untouched")
	eq(files[T], nil, "backup hash mismatch removes the temp file")
end

do
	local fns, files = mk({ live = "good", move_ok = false })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "replace_failed", "failed atomic move aborts")
	eq(files[P], "good", "failed move leaves live untouched")
	eq(files[T], nil, "failed move removes the temp file")
end

do
	local fns = mk({ live = "good" })
	-- md5 returning nil means the checksum itself failed
	fns.md5 = function() return nil end
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "io_error", "unreadable checksum is an io error")
end

do
	local fns, files = mk({ live = "good", write_ok = false })
	local state = apply.write({ path = P, candidate = "new", fns = fns })
	eq(state, "io_error", "unwritable temp file is an io error")
	eq(files[P], "good", "unwritable temp file leaves live untouched")
end

-- ---- restore path ---------------------------------------------------------

do
	local fns, files, calls = mk({ live = "broken" })
	local state = apply.restore({ path = P, fns = fns })
	eq(state, "no_backup", "restore without a .bak reports no_backup")
	eq(files[P], "broken", "restore without backup keeps live")
	eq(find(calls, "restart"), nil, "restore without backup restarts nothing")
end

do
	local fns, files, calls = mk({ live = "broken", bak = "good",
		validate_rc = 1 })
	local state = apply.restore({ path = P, fns = fns })
	eq(state, "invalid", "corrupt backup is refused, not installed")
	eq(files[P], "broken", "refused restore keeps the live file")
	eq(files[B], "good", "refused restore keeps the backup file")
	eq(files[T], nil, "refused restore removes the temp file")
	eq(find(calls, "restart"), nil, "refused restore restarts nothing")
end

do
	local fns, files = mk({ live = "broken", bak = "good", running = true })
	local state = apply.restore({ path = P, fns = fns })
	eq(state, "confirmed", "restore of a valid backup settles confirmed")
	eq(files[P], "good", "restore puts the backup content live")
end

do
	-- restore must NOT copy live over .bak first: that would destroy the
	-- only good copy with the broken current file.
	local fns, files, calls = mk({ live = "broken", bak = "good", running = true })
	apply.restore({ path = P, fns = fns })
	eq(files[B], "good", "restore never clobbers the backup with live")
	eq(find(calls, "copy"), nil, "restore makes no backup copy")
end

do
	local fns, files = mk({ live = "current", bak = "good" })
	local state = apply.restore({ path = P, base_md5 = "wrong", fns = fns })
	eq(state, "stale", "restore rejects a stale baseline md5")
	eq(files[P], "current", "stale restore leaves live untouched")
end

do
	local fns = mk({ live = "broken", bak = "good", enabled = false })
	local state = apply.restore({ path = P, fns = fns })
	eq(state, "stopped_by_user", "restore settles stopped_by_user when disabled")
end

print("\n" .. passed .. " assertions passed")
