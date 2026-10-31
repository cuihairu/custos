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
end
