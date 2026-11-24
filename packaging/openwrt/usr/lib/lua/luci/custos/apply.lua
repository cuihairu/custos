-- Copyright 2026 cuihairu <chuihairu@gmail.com>
-- SPDX-License-Identifier: MIT

-- The TOML write chain (docs/design/settings-redesign/apply-contract.md §2/§3,
-- C-SET-UX-04/05). All side effects are injected through `opts.fns` so the
-- unit tests (packaging/openwrt/tests/apply-test.lua) can drive every path on
-- host Lua; the controller wires the real shell/UCI implementations.
--
-- Contract order (never reordered):
--   ① stale check   client baseline md5 must match the live file, else refuse
--   ② no-op check   candidate byte-identical to live -> "noop", nothing runs
--   ③ candidate     write <path>.new in the same directory (atomic mv later)
--   ④ validate      custos validate on the candidate; failure removes the
--                   temp file and touches NOTHING else (zero side effects:
--                   the old code backed up before validating and destroyed
--                   the last-known-good .bak on failed attempts)
--   ⑤ backup        only after validation passed: copy live -> .bak and
--                   verify the copy (exit code, readable, md5 identical)
--   ⑥ replace       mv .new -> live (atomic, same filesystem)
--   ⑦ settle        restart, then poll the service: "confirmed" when running
--                   within the window, "stopped_by_user" when UCI has the
--                   service disabled, "restart_failed" otherwise
--
-- Failure invariant: on any failure the live file still holds the last good
-- config; .bak is only ever written at step ⑤, immediately before a
-- replacement that has already been validated.

local M = {}

local function normalize_rc(rc)
	if rc == true then
		return 0
	end
	if rc == false or rc == nil then
		return 1
	end
	return rc
end

-- Post-restart confirmation (§3): poll for a running service, then decide
-- between confirmed / stopped_by_user / restart_failed. "written" is the
-- transient the caller shows while this runs; the synchronous API resolves
-- to one of the three final states.
function M.settle(fns, poll)
	local rc = normalize_rc(fns.restart())
	local times = (poll and poll.times) or 10
	local interval = (poll and poll.interval) or 0.5
	if rc == 0 then
		for _ = 1, times do
			if fns.running() then
				return "confirmed"
			end
			fns.sleep(interval)
		end
	end
	if fns.enabled and fns.enabled() == false then
		return "stopped_by_user"
	end
	return "restart_failed"
end

-- Restore the .bak copy over the live file. The backup itself is NOT
-- re-copied (it is the source of truth here; copying live over it first
-- would destroy the only good copy). The candidate still goes through
-- validation: a corrupt backup must be refused, not installed.
-- opts: path, base_md5, fns; returns state, info.
function M.restore(opts)
	local fns = opts.fns
	local path = opts.path
	local bak = path .. ".bak"

	local cur = fns.md5(path)
	if opts.base_md5 and cur ~= opts.base_md5 then
		return "stale", { message = "配置已被其他会话修改，请刷新后重试" }
	end

	local text = fns.read(bak)
	if not text then
		return "no_backup", { message = "没有可用的备份文件 " .. bak }
	end

	local tmp = path .. ".new"
	if not fns.write(tmp, text) then
		return "io_error", { message = "无法写入临时文件" }
	end
	local rc, out = fns.validate(tmp)
	if normalize_rc(rc) ~= 0 then
		fns.rm(tmp)
		return "invalid", { output = out, message = "备份文件未通过校验，未做恢复" }
	end
	if not fns.move(tmp, path) then
		fns.rm(tmp)
		return "replace_failed", { message = "替换配置文件失败，live 未改动" }
	end
	local state = M.settle(fns, opts.poll)
	return state, { backup = bak }
end

-- The write chain. opts: path, candidate, base_md5, fns, poll.
-- Returns state, info. Success-side states (live replaced by a validated
-- candidate): confirmed / stopped_by_user / restart_failed. Rejections
-- (live untouched): stale / noop / io_error / invalid / backup_failed /
-- replace_failed.
function M.write(opts)
	local fns = opts.fns
	local path = opts.path
	local candidate = opts.candidate
	local tmp = path .. ".new"
	local bak = path .. ".bak"

	-- ① concurrency: the caller must have seen the current bytes.
	local cur = fns.md5(path)
	if opts.base_md5 and cur ~= opts.base_md5 then
		return "stale", { message = "配置已被其他会话修改，请刷新后重试" }
	end
	if not cur then
		return "io_error", { message = "无法读取当前配置文件校验和" }
	end

	-- ② no-op: byte-identical candidates skip validate/backup/restart.
	local live = fns.read(path)
	if live == candidate then
		return "noop", { message = "无改动" }
	end

	-- ③ candidate temp file, same directory for the atomic mv.
	if not fns.write(tmp, candidate) then
		return "io_error", { message = "无法写入临时文件" }
	end

	-- ④ validate the candidate; a failure must not touch live or .bak.
	local rc, out = fns.validate(tmp)
	if normalize_rc(rc) ~= 0 then
		fns.rm(tmp)
		return "invalid", { output = out, message = "custos validate 未通过，配置已回滚" }
	end

	-- ⑤ verified backup: copy, then prove the copy (readable + same md5).
	if not fns.copy(path, bak) then
		fns.rm(tmp)
		return "backup_failed", { message = "备份当前配置失败，live 未改动" }
	end
	local bak_md5 = fns.md5(bak)
	if not bak_md5 or bak_md5 ~= cur then
		fns.rm(tmp)
		return "backup_failed", { message = "备份校验和与当前配置不一致，live 未改动" }
	end

	-- ⑥ atomic replace.
	if not fns.move(tmp, path) then
		fns.rm(tmp)
		return "replace_failed", { message = "替换配置文件失败，live 未改动" }
	end

	-- ⑦ restart + poll.
	local state = M.settle(fns, opts.poll)
	return state, { backup = bak }
end

return M
