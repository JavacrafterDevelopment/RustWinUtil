# RustWinUtil

A portable, Rust-built take on [Chris Titus Tech's WinUtil](https://github.com/ChrisTitusTech/winutil):
install apps in bulk, apply and undo Windows tweaks, turn on Windows features, run
common fixes and choose a Windows Update profile, from one window laid out like
WinUtil's, in the cream-walnut-and-gilt look of
[Lighthouse](https://github.com/JavacrafterDevelopment/Lighthouse) and the
[Javacrafter Launcher](https://github.com/JavacrafterDevelopment/Launcher).

It is **not** affiliated with or endorsed by Chris Titus Tech. It works entirely from
a copy of WinUtil's data built into the exe: it never downloads anything from
WinUtil's repository or website.

> Like WinUtil, this changes the registry, services, Windows features and installed
> apps. Most tweaks can be undone, but read what a tweak does before running it, and
> consider a restore point first (Tweaks → *Restore Point - Create*).

---

## Running it

Put the folder anywhere and run `RustWinUtil.exe`. It asks for administrator rights,
as WinUtil does; if you decline it still opens, but most changes will fail. Nothing
is installed, and what it keeps (settings, the scripts it last ran, its browser profile) lives in
`data\` beside the exe.

Only one copy runs per folder: opening it again brings the open window forward.
`RustWinUtil.exe --quit` asks the running copy to close, even when it's running as
administrator; the Javacrafter Launcher's **Stop** button uses it.

It's in the Javacrafter Launcher's library, which installs it, shows its updates and
installs them when you press **Update**.

## What's in it

| Tab | What it does |
|---|---|
| **Install** | All 236 of WinUtil's apps, by category, with filters and search. Install/upgrade, uninstall or upgrade everything with **WinGet** or **Chocolatey**. **Show Installed Apps** ticks what's already there. Open-source apps carry a green dot; double-click a card for the app's website. |
| **Tweaks** | WinUtil's essential and advanced tweaks with its **Standard**, **Minimal** and **Advanced** presets, **Run Tweaks** and **Undo Selected Tweaks**, **Get Installed Tweaks**, the preference toggles (read live from the registry), DNS provider, multiplane overlay, O&O ShutUp10++ and the Ultimate Performance power plan. |
| **AppX Removal** | (from Tweaks) Remove WinUtil's list of built-in Windows apps for every user, or reinstall them from the Microsoft Store. |
| **Config** | Windows features (.NET 3.5, Hyper-V, WSL, Sandbox…), fixes (network reset, Windows Update reset, system corruption scan, WinGet reinstall, AutoLogon, NTP), the legacy Windows control panels, and OpenSSH Server. |
| **Updates** | WinUtil's three Windows Update profiles: Recommended, Windows Default and Disable Updates. |

Every action runs as a job, one at a time, and the log at the bottom shows what it's
doing as it does it.

**Left out** for now:

- **Win11 Creator** (building a customised Windows ISO).
- **CTT PowerShell Profile** install/remove: those download Chris Titus Tech's own
  profile script, and RustWinUtil fetches nothing from WinUtil's sources.
- App icons, which WinUtil downloads from the internet; cards carry the app's initial.

Some actions still need the internet by nature, from their own vendors: installing
apps (WinGet, Chocolatey), O&O ShutUp10++ (from O&O), AutoLogon (from Microsoft's
Sysinternals) and the Adobe URL block list.

## How it works

- **Data.** `winutil/config/` is WinUtil's own JSON (apps, tweaks, features, DNS
  providers, presets, AppX list), copied unchanged from upstream commit
  [`07ccd8e`](winutil/UPSTREAM-COMMIT.txt) and built into the exe.
- **Rust does the work** where it's declarative: registry values (written and read
  natively, including the toggles' live state), service startup types, Windows
  features (DISM), and WinGet / Chocolatey, whose output streams into the log.
- **PowerShell only for WinUtil's scripts.** Tweaks and fixes that carry PowerShell
  run it with WinUtil's own helper functions (`winutil/functions/`, unchanged) and
  RustWinUtil's small stand-ins for the parts of WinUtil's window they expect
  ([src/prelude.ps1](src/prelude.ps1)).
- **The window** is a borderless [tao](https://github.com/tauri-apps/tao) window with
  a [wry](https://github.com/tauri-apps/wry) WebView2 page, built the same way as the
  launcher.

```
src/main.rs       window, settings, messages between page and host
src/engine.rs     the jobs: apps, tweaks, toggles, features, fixes, updates, AppX
src/sys.rs        registry, services, child processes, elevation, one instance
src/prelude.ps1   stand-ins for WinUtil's GUI plumbing in its scripts
ui/               the page (index.html, app.css, app.js), built into the exe
winutil/          WinUtil's config and scripts, with its license
tools/            icon, release packaging and update.json
```

## Building

Needs [Rust](https://rustup.rs). `cargo build --release` gives `target\release\RustWinUtil.exe`.
The GNU toolchain works too (see the Launcher's README for the `dlltool` setup); its
builds also need `WebView2Loader.dll` beside the exe. `cargo test` checks the
config, loads every WinUtil script in PowerShell, and reads tweak state, all without
changing anything.

## Releases

Push a version tag and [the Release workflow](.github/workflows/release.yml) builds
it, packs `RustWinUtil-windows-x64.zip` with a manifest of every file's SHA-256,
publishes the release, and writes `update.json` to the `updates` branch, which is
where the Javacrafter Launcher finds and verifies updates.

```bash
# bump version in Cargo.toml, e.g. to 0.0.2, then:
git commit -am "Release 0.0.2"
git tag v0.0.2
git push origin main v0.0.2
```

## Updating WinUtil's data

To bring in newer apps or tweaks, copy the files in `winutil/config/` and the scripts
in `winutil/functions/` from a newer WinUtil checkout, update
`winutil/UPSTREAM-COMMIT.txt`, run `cargo test`, and release.

## License

RustWinUtil's own code is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

### Third-party components

| Path | From | License |
|---|---|---|
| `winutil/config/`, `winutil/functions/` | [WinUtil](https://github.com/ChrisTitusTech/winutil), Copyright (c) 2022 CT Tech Group LLC | MIT ([winutil/LICENSE](winutil/LICENSE)) |

Release builds also contain the Rust crates in `Cargo.lock` (permissive licenses, mostly
MIT OR Apache-2.0) and carry both RustWinUtil's and WinUtil's license texts in a
`licenses` folder beside the exe.
