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

	entry({"admin", "services", "custos", "api", "status"},
		call("api_status"),
		_("状态 API"), 90).leaf = true
end

function api_status()
	luci.http.prepare_content("application/json")
	-- procd service state via the rc.common wrapper (exit 0 = running,
	-- non-zero = stopped); report the raw code plus whether the config
	-- file is present so the page can distinguish "stopped" from "uninstalled".
	local code = luci.sys.call("/etc/init.d/custos status >/dev/null 2>&1")
	local has_config = nixio.fs.access("/etc/custos/custos.toml") and "yes" or "no"
	luci.http.write_json({
		status = (code == 0) and "running" or "stopped",
		config = has_config,
	})
end
