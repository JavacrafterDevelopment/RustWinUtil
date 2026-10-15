//! What the buttons do. Every job runs on one worker thread, one at a time, and
//! reports to the page as it goes, as WinUtil's own job queue does.
//!
//! The data (apps, tweaks, features, DNS providers, AppX packages, presets) is
//! WinUtil's own config, embedded in the exe. Registry values, service startup
//! types, Windows features and package managers are handled here in Rust; the
//! free-form PowerShell that some tweaks and fixes carry runs through
//! [`run_script`], with WinUtil's helper functions alongside.

use serde::Serialize;
use serde_json::{json, Map, Value};
use std::process::Command;
use std::sync::{mpsc, Arc};

use crate::sys;

pub const APPLICATIONS: &str = include_str!("../winutil/config/applications.json");
pub const TWEAKS: &str = include_str!("../winutil/config/tweaks.json");
pub const FEATURES: &str = include_str!("../winutil/config/feature.json");
pub const DNS: &str = include_str!("../winutil/config/dns.json");
pub const PRESETS: &str = include_str!("../winutil/config/preset.json");
pub const APPX: &str = include_str!("../winutil/config/appx.json");

const PRELUDE: &str = include_str!("prelude.ps1");

/// WinUtil's own scripts, unchanged, made available to every script RustWinUtil runs.
const FUNCTIONS: &[&str] = &[
    include_str!("../winutil/functions/Invoke-WinUtilExplorerUpdate.ps1"),
    include_str!("../winutil/functions/Save-WinUtilFile.ps1"),
    include_str!("../winutil/functions/Invoke-WPFFixesNTPPool.ps1"),
    include_str!("../winutil/functions/Invoke-WPFFixesNetwork.ps1"),
    include_str!("../winutil/functions/Invoke-WPFFixesUpdate.ps1"),
    include_str!("../winutil/functions/Invoke-WPFFixesWinget.ps1"),
    include_str!("../winutil/functions/Invoke-WPFSystemRepair.ps1"),
    include_str!("../winutil/functions/Invoke-WPFSSHServer.ps1"),
    include_str!("../winutil/functions/Invoke-WinUtilSSHServer.ps1"),
    include_str!("../winutil/functions/Invoke-WPFPanelAutologin.ps1"),
    include_str!("../winutil/functions/Invoke-WPFUltimatePerformance.ps1"),
    include_str!("../winutil/functions/Invoke-WPFOOSU.ps1"),
    include_str!("../winutil/functions/Invoke-WPFUpdatesdefault.ps1"),
    include_str!("../winutil/functions/Invoke-WPFUpdatesdisable.ps1"),
    include_str!("../winutil/functions/Invoke-WPFUpdatessecurity.ps1"),
    include_str!("../winutil/functions/Set-WinUtilDNS.ps1"),
    include_str!("../winutil/functions/Get-WinUtilDNSBenchmark.ps1"),
    include_str!("../winutil/functions/Remove-WinUtilAPPX.ps1"),
    include_str!("../winutil/functions/Remove-WinUtilProvisionedAPPX.ps1"),
    include_str!("../winutil/functions/Install-WinUtilWinget.ps1"),
    include_str!("../winutil/functions/Install-WinUtilChoco.ps1"),
    include_str!("../winutil/functions/Test-WinUtilPackageManager.ps1"),
];

/// Feature-panel buttons that call one of WinUtil's functions, and the call.
/// The PowerShell-profile buttons are left out: they download Chris Titus
/// Tech's own scripts, and RustWinUtil fetches nothing from WinUtil's sources.
fn function_call(key: &str) -> Option<&'static str> {
    Some(match key {
        "WPFFixesNTPPool" => "Invoke-WPFFixesNTPPool",
        "WPFFixesNetwork" => "Invoke-WPFFixesNetwork",
        "WPFFixesUpdate" => "Invoke-WPFFixesUpdate",
        "WPFFixesWinget" => "Invoke-WPFFixesWinget",
        "WPFPanelAutologin" => "Invoke-WPFPanelAutologin",
        "WPFPanelDISM" => "Invoke-WPFSystemRepair",
        "WPFWinUtilSSHServer" => "Invoke-WPFSSHServer",
        "WPFOOSUbutton" => "Invoke-WPFOOSU",
        "WPFAddUltPerf" => "Invoke-WPFUltimatePerformance -Enable",
        "WPFRemoveUltPerf" => "Invoke-WPFUltimatePerformance",
        "recommended" => "Invoke-WPFUpdatessecurity",
        "default" => "Invoke-WPFUpdatesdefault",
        "disable" => "Invoke-WPFUpdatesdisable -Confirmed",
        _ => return None,
    })
}

/// The feature-panel entries RustWinUtil doesn't offer. See [`function_call`].
pub const LEFT_OUT: &[&str] = &["WPFWinUtilInstallPSProfile", "WPFWinUtilUninstallPSProfile"];

#[derive(Clone, Copy, PartialEq)]
pub enum Manager {
    Winget,
    Choco,
}

pub enum Job {
    InstallApps(Vec<String>, Manager),
    UninstallApps(Vec<String>, Manager),
    UpgradeAll(Manager),
    DetectApps(Manager),
    RunTweaks(Vec<String>),
    UndoTweaks(Vec<String>),
    DetectTweaks,
    Toggle(String, bool),
    Combo(String, String),
    Features(Vec<String>),
    /// A button that calls one of WinUtil's functions: fixes, OOSU, performance plans, update profiles.
    Function(String, String),
    RemoveAppx(Vec<String>),
    ReinstallAppx(Vec<String>),
    DetectAppx,
    /// Reads every toggle and combo box from the registry.
    RefreshStates,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Log { text: String, kind: &'static str },
    Busy { label: String },
    Progress { done: usize, total: usize },
    Done { label: String, ok: bool },
    InstalledApps { keys: Vec<String> },
    AppliedTweaks { keys: Vec<String> },
    States { toggles: Map<String, Value>, combos: Map<String, Value> },
    InstalledAppx { keys: Vec<String> },
}

pub struct Engine {
    apps: Map<String, Value>,
    tweaks: Map<String, Value>,
    features: Map<String, Value>,
    appx: Map<String, Value>,
    emit: Arc<dyn Fn(Event) + Send + Sync>,
}

/// Reads one of WinUtil's config files. They're written for PowerShell's
/// ConvertFrom-Json, which accepts raw line breaks and tabs inside strings (the
/// multi-line scripts rely on it); strict JSON doesn't, so those are escaped first.
pub fn load(text: &str) -> Value {
    let mut out = String::with_capacity(text.len() + 1024);
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string {
            match c {
                _ if escaped => {
                    escaped = false;
                    out.push(c);
                }
                '\\' => {
                    escaped = true;
                    out.push(c);
                }
                '"' => {
                    in_string = false;
                    out.push(c);
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        } else {
            in_string = c == '"';
            out.push(c);
        }
    }
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("embedded WinUtil config is unreadable: {e}"))
}

fn parse(text: &str) -> Map<String, Value> {
    load(text).as_object().cloned().unwrap_or_default()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Starts the worker; returns where to send jobs.
pub fn start(emit: Arc<dyn Fn(Event) + Send + Sync>) -> mpsc::Sender<Job> {
    let engine = Engine {
        apps: parse(APPLICATIONS),
        tweaks: parse(TWEAKS),
        features: parse(FEATURES),
        appx: parse(APPX),
        emit,
    };
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        for job in rx {
            engine.run(job);
        }
    });
    tx
}

impl Engine {
    fn log(&self, kind: &'static str, text: impl Into<String>) {
        (self.emit)(Event::Log { text: text.into(), kind });
    }

    fn info(&self) -> impl Fn(String) + '_ {
        move |line| self.log("info", line)
    }

    fn progress(&self, done: usize, total: usize) {
        (self.emit)(Event::Progress { done, total });
    }

    fn run(&self, job: Job) {
        let label = match &job {
            Job::InstallApps(..) => "Installing apps",
            Job::UninstallApps(..) => "Uninstalling apps",
            Job::UpgradeAll(_) => "Upgrading all apps",
            Job::DetectApps(_) => "Checking installed apps",
            Job::RunTweaks(_) => "Running tweaks",
            Job::UndoTweaks(_) => "Undoing tweaks",
            Job::DetectTweaks => "Checking applied tweaks",
            Job::Toggle(..) => "Changing a setting",
            Job::Combo(..) => "Changing a setting",
            Job::Features(_) => "Installing features",
            Job::Function(..) => "Working",
            Job::RemoveAppx(_) => "Removing AppX packages",
            Job::ReinstallAppx(_) => "Reinstalling AppX packages",
            Job::DetectAppx => "Checking AppX packages",
            Job::RefreshStates => "Reading settings",
        };
        let label = match &job {
            Job::Function(_, l) => l.clone(),
            _ => label.to_string(),
        };
        let quiet = matches!(job, Job::RefreshStates);
        if !quiet {
            (self.emit)(Event::Busy { label: label.clone() });
            self.log("head", label.clone());
        }
        let result = match job {
            Job::InstallApps(keys, m) => self.packages("install", &keys, m),
            Job::UninstallApps(keys, m) => self.packages("uninstall", &keys, m),
            Job::UpgradeAll(m) => self.upgrade_all(m),
            Job::DetectApps(m) => self.detect_apps(m),
            Job::RunTweaks(keys) => self.tweaks_batch(&keys, false),
            Job::UndoTweaks(keys) => self.tweaks_batch(&keys, true),
            Job::DetectTweaks => self.detect_tweaks(),
            Job::Toggle(key, on) => {
                let r = self.apply_tweak(&key, !on);
                self.send_states();
                r
            }
            Job::Combo(key, state) => {
                let r = self.combo(&key, &state);
                self.send_states();
                r
            }
            Job::Features(keys) => self.install_features(&keys),
            Job::Function(key, _) => self.call_function(&key),
            Job::RemoveAppx(keys) => self.remove_appx(&keys),
            Job::ReinstallAppx(keys) => self.reinstall_appx(&keys),
            Job::DetectAppx => self.detect_appx(),
            Job::RefreshStates => {
                self.send_states();
                Ok(())
            }
        };
        if !quiet {
            match &result {
                Ok(()) => self.log("ok", format!("{label}: done")),
                Err(e) => self.log("error", format!("{label}: {e}")),
            }
            (self.emit)(Event::Done { label, ok: result.is_ok() });
        }
    }

    // ------------------------------------------------------------- packages

    fn package_id(&self, key: &str, m: Manager) -> Option<String> {
        let app = self.apps.get(key)?;
        let raw = s(app, if m == Manager::Winget { "winget" } else { "choco" });
        let id = raw.split(';').next_back().unwrap_or_default().trim().to_string();
        (!id.is_empty() && id != "na").then_some(id)
    }

    fn name(&self, key: &str) -> String {
        self.apps.get(key).map(|a| s(a, "content").to_string()).unwrap_or_else(|| key.to_string())
    }

    fn manager_ready(&self, m: Manager) -> Result<(), String> {
        let (exe, hint) = match m {
            Manager::Winget => ("winget.exe", "Use Config → Fixes → WinGet - Reinstall, or switch to Chocolatey."),
            Manager::Choco => ("choco.exe", "Install Chocolatey from chocolatey.org, or switch to WinGet."),
        };
        let found = Command::new("where.exe").arg(exe).creation_flags_no_window().output().is_ok_and(|o| o.status.success());
        if found { Ok(()) } else { Err(format!("{exe} isn't installed. {hint}")) }
    }

    fn packages(&self, action: &str, keys: &[String], m: Manager) -> Result<(), String> {
        self.manager_ready(m)?;
        let mut failed = 0;
        for (i, key) in keys.iter().enumerate() {
            self.progress(i, keys.len());
            let name = self.name(key);
            let Some(id) = self.package_id(key, m) else {
                self.log("warn", format!("{name} has no package for this package manager; skipped"));
                continue;
            };
            self.log("info", format!("{} {name} ({id})", if action == "install" { "Installing" } else { "Uninstalling" }));
            let code = match m {
                Manager::Winget => {
                    let (id, source) = match id.strip_prefix("msstore:") {
                        Some(store) => (store.to_string(), "msstore"),
                        None => (id.clone(), "winget"),
                    };
                    let mut cmd = Command::new("winget");
                    cmd.args([action, "--id", &id, "--source", source, "--silent", "--disable-interactivity"]);
                    if action == "install" {
                        cmd.args(["--accept-package-agreements", "--accept-source-agreements"]);
                    }
                    sys::run(&mut cmd, &self.info())?
                }
                Manager::Choco => sys::run(Command::new("choco").args([action, &id, "-y", "--no-progress"]), &self.info())?,
            };
            match (m, code) {
                (_, 0) => self.log("ok", format!("{name}: done")),
                // winget: already installed / no newer version / restart needed to finish.
                (Manager::Winget, -1978335189) | (Manager::Winget, -1978335212) => {
                    self.log("ok", format!("{name}: already installed and up to date"))
                }
                (Manager::Winget, -1978334967) | (Manager::Winget, -1978334965) => {
                    self.log("ok", format!("{name}: done; a restart is needed to finish"))
                }
                (Manager::Choco, 3010) => self.log("ok", format!("{name}: done; a restart is needed to finish")),
                _ => {
                    failed += 1;
                    self.log("error", format!("{name}: failed (exit code {code})"));
                }
            }
        }
        self.progress(keys.len(), keys.len());
        if failed > 0 { Err(format!("{failed} of {} didn't finish", keys.len())) } else { Ok(()) }
    }

    fn upgrade_all(&self, m: Manager) -> Result<(), String> {
        self.manager_ready(m)?;
        let code = match m {
            Manager::Winget => sys::run(
                Command::new("winget").args([
                    "upgrade", "--all", "--accept-package-agreements", "--accept-source-agreements",
                    "--include-unknown", "--silent", "--disable-interactivity",
                ]),
                &self.info(),
            )?,
            Manager::Choco => sys::run(Command::new("choco").args(["upgrade", "all", "-y", "--no-progress"]), &self.info())?,
        };
        if code == 0 { Ok(()) } else { Err(format!("finished with exit code {code}")) }
    }

    fn detect_apps(&self, m: Manager) -> Result<(), String> {
        self.manager_ready(m)?;
        let output = match m {
            Manager::Winget => Command::new("winget")
                .args(["list", "--accept-source-agreements", "--disable-interactivity"])
                .creation_flags_no_window()
                .output(),
            Manager::Choco => Command::new("choco").args(["list"]).creation_flags_no_window().output(),
        }
        .map_err(|e| format!("couldn't list installed apps ({e})"))?;
        let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
        let found: Vec<String> = self
            .apps
            .keys()
            .filter(|key| {
                let Some(id) = self.package_id(key, m) else { return false };
                let id = id.trim_start_matches("msstore:").to_lowercase();
                match m {
                    // winget's table: the id stands in its own column, two spaces either side.
                    Manager::Winget => text.lines().any(|l| {
                        l.split("  ").map(str::trim).any(|cell| cell == id)
                    }),
                    Manager::Choco => text.lines().any(|l| l.split_whitespace().next() == Some(id.as_str())),
                }
            })
            .cloned()
            .collect();
        self.log("info", format!("{} of the listed apps are installed", found.len()));
        (self.emit)(Event::InstalledApps { keys: found });
        Ok(())
    }

    // --------------------------------------------------------------- tweaks

    fn tweaks_batch(&self, keys: &[String], undo: bool) -> Result<(), String> {
        let mut failed = 0;
        for (i, key) in keys.iter().enumerate() {
            self.progress(i, keys.len());
            if let Err(e) = self.apply_tweak(key, undo) {
                failed += 1;
                self.log("error", e);
            }
        }
        self.progress(keys.len(), keys.len());
        self.send_states();
        if failed > 0 { Err(format!("{failed} of {} had errors", keys.len())) } else { Ok(()) }
    }

    /// WinUtil's Invoke-WinUtilTweaks: services, then registry, then scripts.
    /// Applying leaves a service alone if someone changed it from its default.
    fn apply_tweak(&self, key: &str, undo: bool) -> Result<(), String> {
        let tweak = self.tweaks.get(key).ok_or_else(|| format!("unknown tweak {key}"))?;
        self.log("info", format!("{} {}", if undo { "Undoing" } else { "Applying" }, s(tweak, "Content")));
        let mut errors = 0;

        for svc in list(tweak, "service") {
            let name = s(&svc, "Name");
            let (target, original) = (s(&svc, "StartupType"), s(&svc, "OriginalType"));
            match sys::service_start_type(name) {
                None => self.log("warn", format!("Service {name} wasn't found")),
                Some(current) if !undo && !sys::start_type_is(&current, original) => {
                    self.log("info", format!("Service {name} is {current}, not the default {original}; leaving it as set"))
                }
                Some(_) => {
                    if let Err(e) = sys::service_set(name, if undo { original } else { target }, &self.info()) {
                        errors += 1;
                        self.log("error", e);
                    }
                }
            }
        }

        for reg in list(tweak, "registry").iter().filter(|r| r.get("Values").is_none()) {
            let value = s(reg, if undo { "OriginalValue" } else { "Value" });
            if reg.get(if undo { "OriginalValue" } else { "Value" }).is_none() {
                continue;
            }
            let (path, name) = (s(reg, "Path"), s(reg, "Name"));
            match sys::reg_set(path, name, s(reg, "Type"), value) {
                Ok(()) if value == "<RemoveEntry>" => self.log("info", format!("Removed {path}\\{name}")),
                Ok(()) => self.log("info", format!("Set {path}\\{name} to {value}")),
                Err(e) => {
                    errors += 1;
                    self.log("error", e);
                }
            }
        }

        for script in list(tweak, if undo { "UndoScript" } else { "InvokeScript" }) {
            if let Some(body) = script.as_str() {
                if let Err(e) = self.run_script(body) {
                    errors += 1;
                    self.log("error", e);
                }
            }
        }
        if errors > 0 { Err(format!("{}: {errors} step(s) failed", s(tweak, "Content"))) } else { Ok(()) }
    }

    /// Whether a registry entry holds its tweak's value, a missing value counting
    /// per its DefaultState (as in WinUtil's Get-WinUtilToggleStatus).
    fn entry_applied(reg: &Value) -> bool {
        let expected = s(reg, "Value");
        match sys::reg_read(s(reg, "Path"), s(reg, "Name")) {
            Some(v) => v.matches(expected),
            None => match s(reg, "DefaultState") {
                "true" => true,
                _ => s(reg, "OriginalValue") == expected,
            },
        }
    }

    fn toggle_on(&self, tweak: &Value) -> bool {
        list(tweak, "registry").iter().all(Self::entry_applied)
    }

    /// The combo-box state whose values all match the registry now.
    fn combo_state(tweak: &Value) -> Option<String> {
        let regs = list(tweak, "registry");
        let states: Vec<String> = regs.first()?.get("Values")?.as_object()?.keys().cloned().collect();
        states.into_iter().find(|state| {
            regs.iter().all(|reg| {
                let default = s(reg, "DefaultValue");
                let configured = reg["Values"][state.as_str()].as_str().unwrap_or_default();
                let expected = if configured == "<RemoveEntry>" { default } else { configured };
                match sys::reg_read(s(reg, "Path"), s(reg, "Name")) {
                    Some(v) => v.matches(expected),
                    None => default == expected,
                }
            })
        })
    }

    fn combo(&self, key: &str, state: &str) -> Result<(), String> {
        if key == "WPFchangedns" {
            self.log("info", format!("Setting DNS to {state}"));
            return self.run_script(&format!("Set-WinUtilDNS -DNSProvider '{}'", state.replace('\'', "''")));
        }
        let tweak = self.tweaks.get(key).ok_or_else(|| format!("unknown setting {key}"))?;
        self.log("info", format!("{}: {state}", s(tweak, "Content")));
        for reg in list(tweak, "registry") {
            let configured = reg["Values"][state].as_str().ok_or_else(|| format!("unknown option {state}"))?;
            let (path, name) = (s(&reg, "Path"), s(&reg, "Name"));
            let exists = sys::reg_read(path, name).is_some();
            if configured != "<RemoveEntry>" || exists {
                sys::reg_set(path, name, s(&reg, "Type"), configured)?;
                self.log("info", format!("Set {path}\\{name} to {configured}"));
            }
        }
        Ok(())
    }

    fn send_states(&self) {
        let mut toggles = Map::new();
        let mut combos = Map::new();
        for (key, tweak) in &self.tweaks {
            match s(tweak, "Type") {
                "Toggle" => {
                    toggles.insert(key.clone(), json!(self.toggle_on(tweak)));
                }
                "Combobox" if key != "WPFchangedns" => {
                    combos.insert(key.clone(), json!(Self::combo_state(tweak)));
                }
                _ => {}
            }
        }
        (self.emit)(Event::States { toggles, combos });
    }

    /// WinUtil's Invoke-WinUtilCurrentSystem for tweaks: every registry value and
    /// service matches what the tweak sets.
    fn detect_tweaks(&self) -> Result<(), String> {
        let applied: Vec<String> = self
            .tweaks
            .iter()
            .filter(|(_, t)| {
                let kind = s(t, "Type");
                let (regs, svcs) = (list(t, "registry"), list(t, "service"));
                if kind == "Combobox" || kind == "Toggle" || (regs.is_empty() && svcs.is_empty()) {
                    return false;
                }
                regs.iter().all(Self::entry_applied)
                    && svcs.iter().all(|svc| {
                        sys::service_start_type(s(svc, "Name")).is_some_and(|cur| sys::start_type_is(&cur, s(svc, "StartupType")))
                    })
            })
            .map(|(k, _)| k.clone())
            .collect();
        self.log("info", format!("{} tweaks are already applied", applied.len()));
        (self.emit)(Event::AppliedTweaks { keys: applied });
        self.send_states();
        Ok(())
    }

    // ----------------------------------------------------- features & fixes

    fn install_features(&self, keys: &[String]) -> Result<(), String> {
        let mut failed = 0;
        for (i, key) in keys.iter().enumerate() {
            self.progress(i, keys.len());
            let Some(entry) = self.features.get(key) else { continue };
            self.log("info", format!("Enabling {}", s(entry, "Content")));
            for feature in list(entry, "feature") {
                let Some(name) = feature.as_str() else { continue };
                let code = sys::run(
                    Command::new("dism.exe").args(["/online", "/enable-feature", &format!("/featurename:{name}"), "/all", "/norestart"]),
                    &self.info(),
                )?;
                // 3010: done, restart needed.
                if code != 0 && code != 3010 {
                    failed += 1;
                    self.log("error", format!("{name}: DISM exit code {code}"));
                }
            }
            for script in list(entry, "InvokeScript") {
                if let Some(body) = script.as_str() {
                    if let Err(e) = self.run_script(body) {
                        failed += 1;
                        self.log("error", e);
                    }
                }
            }
        }
        self.progress(keys.len(), keys.len());
        if failed > 0 { Err(format!("{failed} step(s) failed; a restart may still be needed")) } else { Ok(()) }
    }

    fn call_function(&self, key: &str) -> Result<(), String> {
        let call = function_call(key).ok_or_else(|| format!("RustWinUtil has no action for {key}"))?;
        self.run_script(call)
    }

    // ------------------------------------------------------------------ appx

    fn appx_ids(&self, keys: &[String], field: &str) -> Vec<(String, String)> {
        keys.iter()
            .filter_map(|k| self.appx.get(k).map(|a| (s(a, "Content").to_string(), s(a, field).to_string())))
            .filter(|(_, id)| !id.is_empty())
            .collect()
    }

    fn remove_appx(&self, keys: &[String]) -> Result<(), String> {
        let packages = self.appx_ids(keys, "PackageId");
        let quoted: Vec<String> = packages.iter().map(|(_, id)| format!("'{}'", id.replace('\'', "''"))).collect();
        let mut script = String::new();
        for (name, id) in &packages {
            script.push_str(&format!("Write-Host 'Removing {}'\nRemove-WinUtilAPPX -Name '{}'\n", name.replace('\'', "''"), id.replace('\'', "''")));
        }
        script.push_str(&format!("Remove-WinUtilProvisionedAPPX -PackageList @({})\n", quoted.join(",")));
        self.run_script(&script)?;
        self.detect_appx()
    }

    fn reinstall_appx(&self, keys: &[String]) -> Result<(), String> {
        self.manager_ready(Manager::Winget)?;
        let mut failed = 0;
        for (i, (name, store)) in self.appx_ids(keys, "StoreId").iter().enumerate() {
            self.progress(i, keys.len());
            self.log("info", format!("Reinstalling {name} from the Microsoft Store"));
            let code = sys::run(
                Command::new("winget").args([
                    "install", "--id", store, "--source", "msstore", "--accept-package-agreements",
                    "--accept-source-agreements", "--silent", "--disable-interactivity",
                ]),
                &self.info(),
            )?;
            if code != 0 && code != -1978335189 {
                failed += 1;
                self.log("error", format!("{name}: failed (exit code {code})"));
            }
        }
        self.progress(keys.len(), keys.len());
        self.detect_appx()?;
        if failed > 0 { Err(format!("{failed} didn't reinstall")) } else { Ok(()) }
    }

    fn detect_appx(&self) -> Result<(), String> {
        let output = Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "try { Get-AppxPackage -AllUsers -ErrorAction Stop | ForEach-Object Name } catch { Get-AppxPackage | ForEach-Object Name }"])
            .creation_flags_no_window()
            .output()
            .map_err(|e| format!("couldn't list AppX packages ({e})"))?;
        let names: Vec<String> = String::from_utf8_lossy(&output.stdout).lines().map(|l| l.trim().to_lowercase()).collect();
        let found: Vec<String> = self
            .appx
            .iter()
            .filter(|(_, a)| {
                let id = s(a, "PackageId").to_lowercase();
                !id.is_empty() && names.iter().any(|n| n.contains(&id))
            })
            .map(|(k, _)| k.clone())
            .collect();
        (self.emit)(Event::InstalledAppx { keys: found });
        Ok(())
    }

    // --------------------------------------------------------------- scripts

    /// Runs PowerShell with RustWinUtil's stand-ins and WinUtil's functions loaded,
    /// streaming its output to the log.
    fn run_script(&self, body: &str) -> Result<(), String> {
        let dir = crate::data_dir().join("run");
        std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {} ({e})", dir.display()))?;
        let mut script = PRELUDE.replace("{{DNS_JSON}}", DNS);
        for f in FUNCTIONS {
            script.push('\n');
            script.push_str(f);
        }
        script.push_str("\n\n# ---- the job ----\n");
        script.push_str(body);
        script.push('\n');

        // Windows PowerShell reads a script without a byte-order mark as the ANSI
        // code page, which would mangle anything outside ASCII.
        let path = dir.join("job.ps1");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(script.as_bytes());
        std::fs::write(&path, bytes).map_err(|e| format!("couldn't write {} ({e})", path.display()))?;

        let code = sys::run(
            Command::new("powershell.exe")
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&path)
                .env("RUSTWINUTIL_WORKDIR", &dir),
            &self.info(),
        )?;
        if code == 0 { Ok(()) } else { Err(format!("PowerShell exited with code {code}")) }
    }
}

/// `output()` with no console window flashing up.
trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An engine whose events are kept for the test to look at.
    fn engine() -> (Engine, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let engine = Engine {
            apps: parse(APPLICATIONS),
            tweaks: parse(TWEAKS),
            features: parse(FEATURES),
            appx: parse(APPX),
            emit: Arc::new(move |e| sink.lock().unwrap().push(serde_json::to_string(&e).unwrap())),
        };
        (engine, seen)
    }

    #[test]
    fn every_config_loads() {
        for (name, text) in [("applications", APPLICATIONS), ("tweaks", TWEAKS), ("feature", FEATURES), ("dns", DNS), ("preset", PRESETS), ("appx", APPX)] {
            assert!(load(text).as_object().is_some_and(|m| !m.is_empty()), "{name}.json");
        }
        assert_eq!(parse(TWEAKS).len(), 67);
    }

    /// Loads the stand-ins and every one of WinUtil's functions in PowerShell; a
    /// syntax error in any of them fails here. Changes nothing on the machine.
    #[test]
    fn powershell_functions_load() {
        let (engine, seen) = engine();
        let body = "Write-Host 'loaded'; foreach ($f in 'Invoke-WPFFixesNTPPool','Invoke-WPFSystemRepair','Invoke-WPFOOSU','Set-WinUtilDNS','Remove-WinUtilAPPX','Invoke-WPFUpdatesdisable','Invoke-WinUtilExplorerUpdate') { if (-not (Get-Command $f -ErrorAction SilentlyContinue)) { throw \"missing $f\" } }; Write-Host \"dns providers: $(@($sync.configs.dns.PSObject.Properties).Count)\"";
        engine.run_script(body).unwrap();
        let log = seen.lock().unwrap().join("\n");
        assert!(log.contains("loaded"), "{log}");
        assert!(log.contains("dns providers: "), "{log}");
    }

    /// Reads every toggle, combo box and tweak from the registry. Read-only.
    #[test]
    fn reads_tweak_state() {
        let (engine, seen) = engine();
        engine.detect_tweaks().unwrap();
        let log = seen.lock().unwrap();
        let states = log.iter().find(|e| e.contains("\"states\"")).expect("states event");
        println!("{}", log.iter().find(|e| e.contains("appliedTweaks")).unwrap());
        assert!(states.contains("WPFToggleDarkMode"));
    }

    /// Lists installed apps with winget and AppX packages. Read-only.
    #[test]
    #[ignore]
    fn detects_installed() {
        let (engine, seen) = engine();
        engine.detect_apps(Manager::Winget).unwrap();
        engine.detect_appx().unwrap();
        for e in seen.lock().unwrap().iter().filter(|e| e.contains("installed")) {
            println!("{}", &e[..e.len().min(300)]);
        }
    }
}
