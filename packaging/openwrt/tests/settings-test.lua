-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- Unit tests for the LuCI settings helpers (settings.lua): the
-- seconds<->milliseconds conversion behind the C-SET-UX-01 "engine interval
-- in seconds, millisecond precision" contract, the Telegram pair rule and
-- the effective-notification-source computation behind C-SET-UX-03. The
-- Herald cases cover both engine-side rules: the http/https scheme check and
-- the hardcoded default-blocking SSRF policy (literal loopback/private
-- targets must be refused by the editor before they dead-letter).
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

-- ---- herald url rule (mirrors env_overlay's scheme + SSRF rejection) ------
eq(settings.herald_url_error(""), nil, "herald url absent is fine")
eq(settings.herald_url_error(nil), nil, "herald url nil is fine")
eq(settings.herald_url_error("http://8.8.8.8"), nil, "public http herald url accepted")
eq(settings.herald_url_error("https://herald.example.com/hook"), nil, "public https herald url accepted")
eq(settings.herald_url_error("HTTP://Herald.Example.com"), nil, "scheme case-insensitive")
eq(settings.herald_url_error("https://fc.example.com"), nil, "hostname starting with fc is not IPv6")
eq(settings.herald_url_error("http://172.32.0.1"), nil, "172.32 is outside the private block")
eq(settings.herald_url_error("http://100.128.0.1"), nil, "100.128 is outside CGNAT")
eq(settings.herald_url_error("http://[2001:4860:4860::8888]"), nil, "public IPv6 accepted")
ok(settings.herald_url_error("ftp://herald.example.com") ~= nil, "ftp scheme rejected")
ok(settings.herald_url_error("herald.example.com") ~= nil, "missing scheme rejected")
ok(settings.herald_url_error("http:/herald.example.com") ~= nil, "single slash rejected")
ok(settings.herald_url_error("tcp://127.0.0.1") ~= nil, "tcp scheme rejected")

-- Literal targets the env overlay's SsrfPolicy::default_blocking() refuses.
ok(settings.herald_url_error("http://127.0.0.1:8080") ~= nil, "loopback rejected")
ok(settings.herald_url_error("http://127.0.0.2") ~= nil, "loopback range rejected")
ok(settings.herald_url_error("http://localhost:8080") ~= nil, "localhost rejected")
ok(settings.herald_url_error("http://user:pass@127.0.0.1:8080") ~= nil, "loopback behind userinfo rejected")
ok(settings.herald_url_error("http://0.0.0.0") ~= nil, "unspecified IPv4 rejected")
ok(settings.herald_url_error("http://10.1.2.3") ~= nil, "10/8 rejected")
ok(settings.herald_url_error("http://172.16.0.1") ~= nil, "172.16/12 start rejected")
ok(settings.herald_url_error("http://172.31.255.255") ~= nil, "172.16/12 end rejected")
ok(settings.herald_url_error("http://192.168.1.1") ~= nil, "192.168/16 rejected")
ok(settings.herald_url_error("http://169.254.1.1") ~= nil, "link-local IPv4 rejected")
ok(settings.herald_url_error("http://100.64.0.1") ~= nil, "CGNAT start rejected")
ok(settings.herald_url_error("http://100.127.255.255") ~= nil, "CGNAT end rejected")
ok(settings.herald_url_error("http://[::1]:8080") ~= nil, "IPv6 loopback rejected")
ok(settings.herald_url_error("http://[::]") ~= nil, "IPv6 unspecified rejected")
ok(settings.herald_url_error("http://[0:0:0:0:0:0:0:1]") ~= nil, "long IPv6 loopback rejected")
ok(settings.herald_url_error("http://[fc00::1]") ~= nil, "IPv6 unique-local fc rejected")
ok(settings.herald_url_error("http://[fd12::1]") ~= nil, "IPv6 unique-local fd rejected")
ok(settings.herald_url_error("http://[fe80::1]") ~= nil, "IPv6 link-local rejected")
ok(settings.herald_url_error("http://[febf::1]") ~= nil, "IPv6 link-local end rejected")
ok(settings.herald_url_error("http://[::ffff:192.168.0.1]") ~= nil, "IPv4-mapped private rejected")
ok(settings.herald_url_error("http://[::ffff:127.0.0.1]") ~= nil, "IPv4-mapped loopback rejected")
eq(settings.herald_url_error("http://[::ffff:8.8.8.8]"), nil, "IPv4-mapped public accepted")

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

-- ---- pending source values (form overlay for the after-apply note) --------
local pending = settings.pending_values({
	enabled = "1", local_log = "1", telegram_token = "", telegram_chat = ""
}, { herald_url = "http://8.8.8.8", telegram_token = "SECRET" })
eq(pending.enabled, "1", "submitted enabled wins over stored")
eq(pending.local_log, "1", "submitted local log wins over stored")
eq(pending.herald_url, "http://8.8.8.8", "absent field falls back to stored")
eq(pending.telegram_token, "", "explicit empty token wins over stored secret")
eq(settings.effective_source(pending.enabled, pending.local_log,
	pending.telegram_token, pending.telegram_chat, pending.herald_url), "luci",
	"pending values drive the same selector rule")

print("\nall " .. passed .. " assertions passed")
