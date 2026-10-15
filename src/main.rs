// No console window in release builds; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod sys;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::WindowBuilderExtWindows;
use tao::window::{Icon, ResizeDirection, Window, WindowBuilder};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

use engine::{Job, Jobs, Manager};

const INDEX_HTML: &str = include_str!("../ui/index.html");
const APP_CSS: &str = include_str!("../ui/app.css");
const APP_JS: &str = include_str!("../ui/app.js");
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon-64.rgba");
const APP_ICONS: &str = include_str!("../assets/app-icons.json");

const PAPER_LIGHT: (u8, u8, u8, u8) = (0xF4, 0xEE, 0xE2, 0xFF);
const PAPER_DARK: (u8, u8, u8, u8) = (0x12, 0x12, 0x12, 0xFF);

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The folder the running executable sits in. Everything RustWinUtil keeps lives
/// beside it, so the folder can be carried around.
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn data_dir() -> PathBuf {
    app_dir().join("data")
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    /// "light" or "dark".
    theme: String,
    /// "winget" or "choco".
    package_manager: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: "dark".into(), package_manager: "winget".into() }
    }
}

impl Settings {
    fn path() -> PathBuf {
        data_dir().join("settings.json")
    }
    fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(Self::path(), serde_json::to_vec_pretty(self).unwrap_or_default());
    }
    fn is_dark(&self) -> bool {
        self.theme == "dark"
    }
    fn manager(&self) -> Manager {
        if self.package_manager == "choco" { Manager::Choco } else { Manager::Winget }
    }
}

enum UserEvent {
    Ipc(String),
    /// From the engine: already JSON for the page.
    Engine(String),
    /// The engine has nothing queued or running any more.
    Idle,
    /// Another launch asked this copy to come forward.
    Show,
    /// `RustWinUtil.exe --quit` (the launcher's Stop button) asked this copy to exit.
    Quit,
}

/// `RustWinUtil.exe --quit`'s exit codes, for the launcher.
mod quit {
    /// The running copy was asked to exit.
    pub const ASKED: i32 = 0;
    /// No copy is running from this folder.
    pub const NOT_RUNNING: i32 = 1;
    /// The running copy is in the middle of a job, so instead of exiting it's asking
    /// in its own window whether to close once the job is done.
    pub const BUSY: i32 = 2;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Asks the running copy to exit, without a window or an elevation prompt.
    if args.iter().any(|a| a == "--quit") {
        let busy = sys::is_busy();
        std::process::exit(match sys::signal("Quit") {
            false => quit::NOT_RUNNING,
            true if busy => quit::BUSY,
            true => quit::ASKED,
        });
    }
    // Already running from this folder: bring that copy forward instead.
    if sys::signal("Show") {
        return;
    }
    // WinUtil's work needs administrator. If the prompt is declined, carry on and
    // say so in the window rather than vanish.
    if !sys::is_elevated() && !args.iter().any(|a| a == "--no-elevate") && sys::relaunch_elevated() {
        return;
    }
    // Two launches at once can both get this far; the second one yields.
    let Some(instance) = sys::claim() else {
        sys::signal("Show");
        return;
    };

    let fixed = app_dir().join("WebView2");
    if fixed.join("msedgewebview2.exe").exists() {
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &fixed);
    }

    let settings = Settings::load();
    // Debug builds can save the assembled page, to open it in a browser's devtools.
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("RUSTWINUTIL_DUMP_HTML") {
            let _ = std::fs::write(path, page_html(&settings));
        }
    }
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("RustWinUtil")
        .with_window_classname("JavacrafterRustWinUtil")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0))
        .with_decorations(false)
        .with_undecorated_shadow(true)
        .with_visible(false)
        .with_window_icon(Icon::from_rgba(ICON_RGBA.to_vec(), 64, 64).ok())
        .build(&event_loop)
        .expect("create window");

    // WebView2 won't share a profile folder between an administrator run and a
    // normal one, so the rare run without elevation gets its own.
    let profile = if sys::is_elevated() { "webview" } else { "webview-limited" };
    let mut web_context = WebContext::new(Some(data_dir().join(profile)));
    let ipc_proxy = proxy.clone();
    let first_load = Cell::new(true);
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_html(page_html(&settings))
        .with_background_color(if settings.is_dark() { PAPER_DARK } else { PAPER_LIGHT })
        .with_devtools(cfg!(debug_assertions))
        .with_browser_accelerator_keys(false)
        // Nothing but RustWinUtil's own page is ever shown, so no other page can reach
        // its commands, which run as administrator. The first load is that page,
        // however WebView2 names a page loaded from a string; after it, nothing from
        // the web or the disk.
        .with_navigation_handler(move |uri| first_load.replace(false) || is_local_page(&uri))
        // Files or links dropped on the window would otherwise be opened in it.
        .with_drag_drop_handler(|_| true)
        .with_ipc_handler(move |req| {
            // A page loaded from a string sends from about:blank, which has no scheme.
            // A page from the web has one; file: and data: addresses don't parse, and
            // wry drops those messages itself.
            if req.uri().scheme().is_none() {
                let _ = ipc_proxy.send_event(UserEvent::Ipc(req.into_body()));
            }
        })
        .build(&window);
    let webview = match webview {
        Ok(w) => w,
        Err(e) => {
            eprintln!("WebView2 couldn't start: {e}");
            return;
        }
    };

    let busy = instance.busy_flag();
    let (show_proxy, quit_proxy) = (proxy.clone(), proxy.clone());
    instance.listen(
        move || {
            let _ = show_proxy.send_event(UserEvent::Show);
        },
        move || {
            let _ = quit_proxy.send_event(UserEvent::Quit);
        },
    );

    let engine_proxy = proxy.clone();
    let jobs = engine::start(Arc::new(move |event| {
        let idle = matches!(event, engine::Event::Idle);
        if let Ok(text) = serde_json::to_string(&event) {
            let _ = engine_proxy.send_event(UserEvent::Engine(text));
        }
        if idle {
            let _ = engine_proxy.send_event(UserEvent::Idle);
        }
    }));

    let mut host = Host { window, webview, settings, jobs, busy, exit_when_idle: false };
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &web_context;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Quit) => {
                if host.close_requested() {
                    *flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                host.send(json!({ "type": "window", "maximized": host.window.is_maximized() }));
            }
            Event::UserEvent(UserEvent::Show) => host.show(),
            Event::UserEvent(UserEvent::Engine(text)) => {
                let _ = host.webview.evaluate_script(&format!("window.onHost && window.onHost({text})"));
            }
            // A job may have been queued since the engine went idle; then it isn't.
            Event::UserEvent(UserEvent::Idle) if !host.jobs.busy() => {
                host.busy.set(false);
                if host.exit_when_idle {
                    *flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::Ipc(body)) => {
                if let Ok(msg) = serde_json::from_str::<Value>(&body) {
                    if host.on_message(&msg) {
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
    });
}

/// Whether a navigation stays on a page loaded from a string: about: or data:,
/// never a page from the web or the disk.
fn is_local_page(uri: &str) -> bool {
    let uri = uri.trim_start().to_ascii_lowercase();
    uri.starts_with("about:") || uri.starts_with("data:")
}

/// The page is inlined into one document with the theme already stamped on, so a
/// dark install never flashes light on the way in. WinUtil's config goes along
/// as data; the page builds the tabs from it, as WinUtil builds its own.
fn page_html(settings: &Settings) -> String {
    let downloads: serde_json::Map<String, Value> = engine::DOWNLOADS
        .iter()
        .map(|(key, who, url)| (key.to_string(), json!({ "who": who, "url": url })))
        .collect();
    let boot = json!({
        "version": VERSION,
        "settings": settings,
        "elevated": sys::is_elevated(),
        "apps": engine::load(engine::APPLICATIONS),
        "tweaks": engine::load(engine::TWEAKS),
        "features": engine::load(engine::FEATURES),
        "dns": engine::load(engine::DNS),
        "presets": engine::load(engine::PRESETS),
        "appx": engine::load(engine::APPX),
        // Fetched once by tools/fetch-icons.ps1 and built in: nothing is loaded at run time.
        "icons": serde_json::from_str::<Value>(APP_ICONS).unwrap_or_default(),
        "downloads": downloads,
        "comingSoon": engine::COMING_SOON,
    });
    // `</` inside the data would end the script element early.
    let boot = boot.to_string().replace("</", "<\\/");
    INDEX_HTML
        .replace("{{THEME}}", if settings.is_dark() { "dark" } else { "light" })
        .replace("/*{{CSS}}*/", APP_CSS)
        .replace("/*{{BOOT}}*/", &format!("window.BOOT = {boot};"))
        .replace("/*{{JS}}*/", APP_JS)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// What the event loop looks after: the window, the page, and the jobs it sends.
struct Host {
    window: Window,
    webview: WebView,
    settings: Settings,
    jobs: Jobs,
    /// Mirrors `jobs.busy()` for `RustWinUtil.exe --quit` to read.
    busy: sys::Busy,
    /// "Close when done" was picked: the window is hidden, and RustWinUtil exits
    /// once nothing is queued or running.
    exit_when_idle: bool,
}

impl Host {
    fn send(&self, msg: Value) {
        let _ = self.webview.evaluate_script(&format!("window.onHost && window.onHost({msg})"));
    }

    fn show(&mut self) {
        if self.exit_when_idle {
            // Opened again while waiting to close: it stays open after all.
            self.exit_when_idle = false;
            self.send(json!({ "type": "log", "kind": "info", "text": "RustWinUtil will stay open." }));
        }
        self.window.set_minimized(false);
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// ✕, Alt+F4 or `--quit`. True when RustWinUtil should exit now. With work under
    /// way it asks first instead, as WinUtil does: exiting would cut the job off
    /// partway, leaving Windows half changed, and drop whatever is queued after it.
    fn close_requested(&mut self) -> bool {
        if !self.jobs.busy() {
            return true;
        }
        // Already closing once the work is done: nothing to ask.
        if !self.exit_when_idle {
            self.show();
            self.send(json!({ "type": "closeWhileBusy" }));
        }
        false
    }

    fn queue(&mut self, job: Job) {
        match self.jobs.send(job) {
            Ok(()) => self.busy.set(self.jobs.busy()),
            Err(()) => self.send(json!({
                "type": "log",
                "kind": "warn",
                "open": true,
                "text": "Another job is still running. Wait for it to finish, then try again."
            })),
        }
    }

    /// Handles one message from the page. Returns true when RustWinUtil should exit.
    fn on_message(&mut self, msg: &Value) -> bool {
        let str_of = |k: &str| msg[k].as_str().unwrap_or_default().to_string();
        match msg["cmd"].as_str().unwrap_or_default() {
            "ready" => {
                self.window.set_visible(true);
                self.window.set_focus();
                self.queue(Job::RefreshStates);
            }
            "window" => match msg["action"].as_str().unwrap_or_default() {
                "drag" => {
                    let _ = self.window.drag_window();
                }
                "minimize" => self.window.set_minimized(true),
                "maximize" => self.window.set_maximized(!self.window.is_maximized()),
                "close" => return self.close_requested(),
                "closeWhenDone" => {
                    if !self.jobs.busy() {
                        return true;
                    }
                    self.exit_when_idle = true;
                    self.window.set_visible(false);
                }
                _ => {}
            },
            "resize" => {
                let dir = match msg["dir"].as_str().unwrap_or_default() {
                    "n" => ResizeDirection::North,
                    "s" => ResizeDirection::South,
                    "e" => ResizeDirection::East,
                    "w" => ResizeDirection::West,
                    "ne" => ResizeDirection::NorthEast,
                    "nw" => ResizeDirection::NorthWest,
                    "se" => ResizeDirection::SouthEast,
                    "sw" => ResizeDirection::SouthWest,
                    _ => return false,
                };
                if !self.window.is_maximized() {
                    let _ = self.window.drag_resize_window(dir);
                }
            }
            "settings" => {
                if let Some(t) = msg["theme"].as_str() {
                    self.settings.theme = if t == "light" { "light".into() } else { "dark".into() };
                }
                if let Some(m) = msg["packageManager"].as_str() {
                    self.settings.package_manager = if m == "choco" { "choco".into() } else { "winget".into() };
                }
                self.settings.save();
                self.send(json!({ "type": "settings", "settings": self.settings }));
            }
            "job" => {
                let keys = strings(&msg["keys"]);
                let m = self.settings.manager();
                let job = match msg["kind"].as_str().unwrap_or_default() {
                    "install" => Job::InstallApps(keys, m),
                    "uninstall" => Job::UninstallApps(keys, m),
                    "upgradeAll" => Job::UpgradeAll(m),
                    "detectApps" => Job::DetectApps(m),
                    "runTweaks" => Job::RunTweaks(keys, str_of("dns")),
                    "undoTweaks" => Job::UndoTweaks(keys),
                    "detectTweaks" => Job::DetectTweaks,
                    "toggle" => Job::Toggle(str_of("key"), msg["on"].as_bool().unwrap_or(false)),
                    "combo" => Job::Combo(str_of("key"), str_of("state")),
                    "features" => Job::Features(keys),
                    "function" => Job::Function(str_of("key"), str_of("label")),
                    "removeAppx" => Job::RemoveAppx(keys),
                    "reinstallAppx" => Job::ReinstallAppx(keys),
                    "detectAppx" => Job::DetectAppx,
                    _ => return false,
                };
                self.queue(job);
            }
            "panel" => {
                if let Err(e) = open_panel(&str_of("key")) {
                    self.send(json!({ "type": "log", "text": e, "kind": "error" }));
                }
            }
            "openUrl" => {
                // Only links RustWinUtil itself shows: app websites from WinUtil's list,
                // the download sources, and AppX packages' Microsoft Store pages. Never
                // arbitrary strings.
                let url = str_of("url");
                let app_link = engine::load(engine::APPLICATIONS)
                    .as_object()
                    .cloned()
                    .is_some_and(|apps| apps.values().any(|a| a["link"].as_str() == Some(url.as_str())));
                let download = engine::DOWNLOADS.iter().any(|(_, _, u)| *u == url);
                let store_page = engine::load(engine::APPX).as_object().cloned().is_some_and(|appx| {
                    appx.values().any(|a| {
                        a["StoreId"].as_str().is_some_and(|id| url == format!("https://apps.microsoft.com/detail/{id}"))
                    })
                });
                let known = app_link || download || store_page;
                if known && (url.starts_with("https://") || url.starts_with("http://")) {
                    let _ = sys::shell_open(&url, None);
                }
            }
            "openDataDir" => {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = sys::shell_open(&data_dir().to_string_lossy(), None);
            }
            _ => {}
        }
        false
    }
}

/// The Legacy Windows Panels buttons: each names a control-panel applet, snap-in or
/// shell location. Opened directly, the way WinUtil hands them to Windows.
fn open_panel(key: &str) -> Result<(), String> {
    let features = engine::load(engine::FEATURES);
    if features[key]["category"].as_str() != Some("Legacy Windows Panels") || features[key].get("function").is_some() {
        return Err(format!("{key} isn't a panel"));
    }
    let script = features[key]["InvokeScript"][0].as_str().ok_or_else(|| format!("{key} isn't a panel"))?.trim();
    let target = match script.strip_prefix("Start-Process") {
        Some(rest) => rest.trim().trim_matches(|c| c == '\'' || c == '"').to_string(),
        None => script.to_string(),
    };
    let (file, args) = match target.split_once(' ') {
        Some((f, a)) if !f.contains('\\') && !f.starts_with("shell:") => (f.to_string(), Some(a.trim().to_string())),
        _ => (target.clone(), None),
    };
    sys::shell_open(&file, args.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the navigation and message guards let through.
    #[test]
    fn only_rustwinutils_own_page() {
        assert!(is_local_page("about:blank"));
        assert!(is_local_page("data:text/html;charset=utf-8;base64,PGh0bWw+"));
        assert!(!is_local_page("https://example.com/"));
        assert!(!is_local_page("file:///C:/Users/me/Downloads/page.html"));
        // The message guard reads wry's http::Uri: the page's about:blank has no
        // scheme, a page from the web does.
        assert!("about:blank".parse::<wry::http::Uri>().unwrap().scheme().is_none());
        assert!("https://example.com/".parse::<wry::http::Uri>().unwrap().scheme().is_some());
    }

    /// Only the Legacy Windows Panels open as panels; other entries' scripts don't.
    #[test]
    fn panels_only() {
        assert!(open_panel("WPFFeatureEnableLegacyRecovery").is_err());
        assert!(open_panel("WPFFixesUpdate").is_err());
    }
}
