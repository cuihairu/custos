-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Unit tests for the LuCI settings helpers (settings.lua): the
-- seconds<->milliseconds conversion behind the C-SET-UX-01 "engine interval
-- in seconds, millisecond precision" contract, the Telegram pair rule and
-- the effective-notification-source computation behind C-SET-UX-03.
-- Run:  lua packaging/openwrt/tests/settings-test.lua

local function script_dir()
	local src = debug.getinfo(1, "S").source:sub(2)
	return src:match("^(.*)/[^/]+$") or "."
end

local base = script_dir() .. "/.."
package.path = base .. "/usr/lib/lua/luci/custos/?.lua;" .. package.path

local settings = require "settings"

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
	ok(a == b, name .. " (" .. tostring(a) .. " == " .. tostring(b) .. ")")
end

-- ---- seconds -> milliseconds --------------------------------------------
eq(settings.to_ms("5"), 5000, "to_ms whole seconds")
eq(settings.to_ms("1.5"), 1500, "to_ms 1.5s round-trip target")
eq(settings.to_ms("0.001"), 1, "to_ms millisecond floor")
eq(settings.to_ms("0.05"), 50, "to_ms two-decimal fraction")
eq(settings.to_ms(" 2 "), 2000, "to_ms trims whitespace")
eq(settings.to_ms("120"), 120000, "to_ms minutes-scale value")
eq(settings.to_ms("0"), nil, "to_ms zero rejected")
eq(settings.to_ms("-1"), nil, "to_ms negative rejected")
eq(settings.to_ms(""), nil, "to_ms empty rejected")
eq(settings.to_ms("abc"), nil, "to_ms non-numeric rejected")
eq(settings.to_ms("1.5004"), nil, "to_ms sub-millisecond residue rejected")
eq(settings.to_ms("1.5s"), nil, "to_ms unit suffix rejected")
eq(settings.to_ms(nil), nil, "to_ms nil rejected")
eq(settings.to_ms(5), nil, "to_ms number type rejected (form values are strings)")

-- ---- milliseconds -> seconds --------------------------------------------
eq(settings.to_seconds(5000), "5", "to_seconds whole")
eq(settings.to_seconds(1500), "1.5", "to_seconds trailing zero trimmed")
eq(settings.to_seconds(1), "0.001", "to_seconds millisecond floor")
eq(settings.to_seconds(150), "0.15", "to_seconds keeps intermediate zeros")
eq(settings.to_seconds(0), nil, "to_seconds zero rejected")
eq(settings.to_seconds(-5), nil, "to_seconds negative rejected")
eq(settings.to_seconds(1.5), nil, "to_seconds fractional ms rejected")

-- The contract only ever passes UCI strings to to_seconds; assert coercion.
eq(settings.to_seconds("1500"), "1.5", "to_seconds accepts UCI string values")

-- ---- canonical round trip (1500 ms is the C-SET-UX-01 acceptance value) --
eq(settings.to_ms(settings.to_seconds(1500)), 1500, "round trip 1500 ms")
eq(settings.to_seconds(settings.to_ms("1.5")), "1.5", "round trip 1.5 s")
eq(settings.to_ms(settings.to_seconds(1)), 1, "round trip 1 ms")
eq(settings.to_seconds(settings.to_ms("0.001")), "0.001", "round trip 0.001 s")

-- ---- telegram pair rule ---------------------------------------------------
eq(settings.telegram_pair_error("", ""), nil, "pair absent is fine")
eq(settings.telegram_pair_error(nil, nil), nil, "pair nil is fine")
eq(settings.telegram_pair_error("tok", "chat"), nil, "pair complete is fine")
ok(settings.telegram_pair_error("tok", "") ~= nil, "token without chat rejected")
ok(settings.telegram_pair_error("", "chat") ~= nil, "chat without token rejected")
ok(settings.telegram_pair_error("tok", nil) ~= nil, "nil chat with token rejected")
ok(settings.telegram_pair_error(nil, "chat") ~= nil, "nil token with chat rejected")

-- ---- effective notification source (mirrors env_overlay + notify_pairs) --
eq(settings.effective_source("0", "1", "t", "c", "http://x"), "toml",
	"override off means TOML even with channels")
eq(settings.effective_source("1", "", "", "", ""), "toml",
	"override on without any channel means TOML")
eq(settings.effective_source("1", "1", "", "", ""), "luci",
	"local log alone is a selector")
eq(settings.effective_source("1", "", "t", "c", ""), "luci",
	"complete telegram pair is a selector")
eq(settings.effective_source("1", "", "t", "", ""), "toml",
	"half telegram pair ignored (init.d drops it)")
eq(settings.effective_source("1", "", "", "", "http://x"), "luci",
	"herald url alone is a selector")
eq(settings.effective_source("1", "", "", "", ""), "toml",
	"no selector means TOML")
eq(settings.effective_source(1, 1, nil, nil, nil), "luci",
	"numeric enabled accepted (UCI may store booleans as numbers)")

-- ---- source note wording --------------------------------------------------
ok(settings.source_note("1", "1", "", "", "", false):find("当前生效", 1, true) ~= nil,
	"note says 当前生效 for stored values")
ok(settings.source_note("1", "1", "", "", "", true):find("应用后", 1, true) ~= nil,
	"note says 应用后 for pending values")
ok(settings.source_note("0", "", "", "", "", false):find("TOML", 1, true) ~= nil,
	"toml note names the TOML section")
ok(settings.source_note("1", "", "", "", "", false):find("半配", 1, true) ~= nil,
	"override-without-channel note explains the half-pair rule")

print("\nall " .. passed .. " assertions passed")
