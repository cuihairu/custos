# CUSTOS-MSI-001 — Windows real-machine install verification

Date: 2026-11-08. Machine: `imac` (Windows 11 Pro 64-bit, build 10.0.26300).
Method: native PowerShell via OpenSSH (no bash). The SSH session is
non-interactive, so desktop screenshots were captured through a scheduled
task with `/it` running in the interactive console session (user `cui`,
session 20).

## Artifacts verified

- New full-product MSI: `custos-0.1.0-x64.msi` (7,942,144 bytes; x64;1033;
  Subject `Custos`) — built by nightly run 42397054170.
- Old published nightly MSI: `custos-cli-0.1.0-x86_64.msi` (7,778,304 bytes)
  — built by nightly run 41830929193 (pre-CUSTOS-MSI-001 layout).

Both share `UpgradeCode=F8D4A5C3-2B9E-4D8A-BE7C-6E1F8D3A7B42` (verified in
the wxs at both build commits), so the new MSI overlays the old as an in-place
upgrade.

## Phases and results

### A. Baseline — install old published nightly

Pre-clean removed a pre-existing Custos install (`{ED899C34-…}`, uninstall
exit 0). Then `msiexec /i custos-cli-0.1.0-x86_64.msi /qn` (exit 0). Baseline
layout confirmed:

| Check | Result |
|-------|--------|
| `C:\Program Files\Custos\custos.exe` | present |
| `C:\Program Files\Custos\custos-tray.exe` | present |
| `LICENSE`, `docs`, `examples` | present |
| Start Menu `Custos` folder | absent (old layout had no shortcuts) |
| HKCU Run `custos-tray` | absent (old layout had no autostart) |
| Product registered | `Custos` ver 0.1.0 |

### B. Overlay upgrade — old → new (same UpgradeCode)

`msiexec /i custos-0.1.0-x64.msi /qn` (exit 0; log `new-install.log` shows
`Installation completed successfully`, `MainEngineThread is returning 0`).
Product code changed `{ED899C34-…}` → `{CF1414FC-0A8E-4487-A049-F20DDE7AE4E8}`
(an in-place upgrade, not a side-by-side install).

| Check | Result |
|-------|--------|
| `custos.exe` / `custos-tray.exe` | present (upgraded in place) |
| `LICENSE`, `docs`, `examples` | present |
| Start Menu `Custos` folder | present |
| `Custos CLI` shortcut | present → target `C:\Program Files\Custos\custos.exe`, icon `CustosIcon` |
| `Custos Tray` shortcut | present → target `C:\Program Files\Custos\custos-tray.exe`, icon `CustosIcon` |
| HKCU `...\CurrentVersion\Run\custos-tray` | present, value `C:\Program Files\Custos\custos-tray.exe` (AUTOTRAY default 1) |
| Product registered | `Custos` ver 0.1.0 |

### C. Uninstall — residue cleanup

`msiexec /x {CF1414FC-…} /qn` (exit 0; log `uninstall.log`).

| Check | Result |
|-------|--------|
| `C:\Program Files\Custos` | **gone** |
| Start Menu `Custos` folder | **gone** |
| HKCU Run `custos-tray` | **gone** |
| Machine `PATH` contains `C:\Program Files\Custos` | **no** |
| Product record | **gone** |

### D. AUTOTRAY=0 opt-out

`msiexec /i custos-0.1.0-x64.msi AUTOTRAY=0 /qn` (exit 0; log
`autotray0-install.log`). Files install but:

| Check | Result |
|-------|--------|
| HKCU Run `custos-tray` | **absent** (opt-out honored) |
| `custos.exe` / `custos-tray.exe` | present |
| cleanup uninstall | exit 0, install folder gone |

## Screenshots

- `startmenu.png` — Explorer at `C:\ProgramData\Microsoft\Windows\Start
  Menu\Programs\Custos` showing the **Custos CLI** and **Custos Tray**
  shortcuts.
- `installdir.png` — Explorer at `C:\Program Files\Custos` showing
  `custos.exe` and `custos-tray.exe` (with the Custos icon), plus `docs`,
  `LICENSE`, `README.md`, `README.zh.md`.

Both captured in the interactive console session (6293×2560 PNG).

## Evidence files

| File | What it proves |
|------|----------------|
| `old-install.log` | msiexec `/l*v` of the baseline install (exit 0) |
| `new-install.log` | msiexec `/l*v` of the overlay upgrade (success) |
| `uninstall.log` | msiexec `/l*v` of the uninstall (exit 0) |
| `autotray0-install.log` | msiexec `/l*v` of the AUTOTRAY=0 install |
| `Custos CLI.lnk` / `Custos Tray.lnk` | the actual Start Menu shortcuts (targets resolved via WScript.Shell: `custos.exe` / `custos-tray.exe`, icon `CustosIcon`) |
| `startmenu.png` / `installdir.png` | visual confirmation of both locations |

## Conclusion

All CUSTOS-MSI-001 requirements verified on a real Windows 11 machine:
in-place overlay upgrade (old→new, same UpgradeCode), two Start Menu shortcut
groups with the Custos icon, HKCU tray autostart (default on, `AUTOTRAY=0`
opt-out), and a clean uninstall with no residue (files, shortcuts, Run key,
PATH, product record).
