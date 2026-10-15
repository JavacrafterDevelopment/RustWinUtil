// No console window in release builds; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod sys;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::WindowBuilderExtWindows;
use tao::window::{Icon, ResizeDirection, Window, WindowBuilder};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

use engine::{Job, Manager};

const INDEX_HTML: &str = include_str!("../ui/index.html");
const APP_CSS: &str = include_str!("../ui/app.css");
const APP_JS: &str = include_str!("../ui/app.js");
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon-64.rgba");

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
    /// Another launch asked this copy to come forward.
    Show,
    /// `RustWinUtil.exe --quit` (the launcher's Stop button) asked this copy to exit.
    Quit,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Asks the running copy to exit, without a window or an elevation prompt.
    if args.iter().any(|a| a == "--quit") {
        sys::signal("Quit");
        return;
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

    let mut settings = Settings::load();
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
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_html(page_html(&settings))
        .with_background_color(if settings.is_dark() { PAPER_DARK } else { PAPER_LIGHT })
        .with_devtools(cfg!(debug_assertions))
        .with_browser_accelerator_keys(false)
        .with_ipc_handler(move |req| {
            let _ = ipc_proxy.send_event(UserEvent::Ipc(req.into_body()));
        })
        .build(&window);
    let webview = match webview {
        Ok(w) => w,
        Err(e) => {
            eprintln!("WebView2 couldn't start: {e}");
            return;
        }
    };

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
        if let Ok(text) = serde_json::to_string(&event) {
            let _ = engine_proxy.send_event(UserEvent::Engine(text));
        }
    }));

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &web_context;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Quit) => {
                *flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                send(&webview, json!({ "type": "window", "maximized": window.is_maximized() }));
            }
            Event::UserEvent(UserEvent::Show) => {
                window.set_minimized(false);
                window.set_visible(true);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::Engine(text)) => {
                let _ = webview.evaluate_script(&format!("window.onHost && window.onHost({text})"));
            }
            Event::UserEvent(UserEvent::Ipc(body)) => {
                if let Ok(msg) = serde_json::from_str::<Value>(&body) {
                    if on_message(&msg, &window, &webview, &mut settings, &jobs) {
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
    });
}

/// The page is inlined into one document with the theme already stamped on, so a
/// dark install never flashes light on the way in. WinUtil's config goes along
/// as data; the page builds the tabs from it, as WinUtil builds its own.
fn page_html(settings: &Settings) -> String {
    let mut features = engine::load(engine::FEATURES);
    if let Some(map) = features.as_object_mut() {
        for key in engine::LEFT_OUT {
            map.remove(*key);
        }
    }
    let boot = json!({
        "version": VERSION,
        "settings": settings,
        "elevated": sys::is_elevated(),
        "apps": engine::load(engine::APPLICATIONS),
        "tweaks": engine::load(engine::TWEAKS),
        "features": features,
        "dns": engine::load(engine::DNS),
        "presets": engine::load(engine::PRESETS),
        "appx": engine::load(engine::APPX),
    });
    // `</` inside the data would end the script element early.
    let boot = boot.to_string().replace("</", "<\\/");
    INDEX_HTML
        .replace("{{THEME}}", if settings.is_dark() { "dark" } else { "light" })
        .replace("/*{{CSS}}*/", APP_CSS)
        .replace("/*{{BOOT}}*/", &format!("window.BOOT = {boot};"))
        .replace("/*{{JS}}*/", APP_JS)
}

fn send(webview: &WebView, msg: Value) {
    let _ = webview.evaluate_script(&format!("window.onHost && window.onHost({msg})"));
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Handles one message from the page. Returns true when RustWinUtil should exit.
fn on_message(msg: &Value, window: &Window, webview: &WebView, settings: &mut Settings, jobs: &mpsc::Sender<Job>) -> bool {
    let str_of = |k: &str| msg[k].as_str().unwrap_or_default().to_string();
    match msg["cmd"].as_str().unwrap_or_default() {
        "ready" => {
            window.set_visible(true);
            window.set_focus();
            let _ = jobs.send(Job::RefreshStates);
        }
        "window" => match msg["action"].as_str().unwrap_or_default() {
            "drag" => {
                let _ = window.drag_window();
            }
            "minimize" => window.set_minimized(true),
            "maximize" => window.set_maximized(!window.is_maximized()),
            "close" => return true,
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
            if !window.is_maximized() {
                let _ = window.drag_resize_window(dir);
            }
        }
        "settings" => {
            if let Some(t) = msg["theme"].as_str() {
                settings.theme = if t == "light" { "light".into() } else { "dark".into() };
            }
            if let Some(m) = msg["packageManager"].as_str() {
                settings.package_manager = if m == "choco" { "choco".into() } else { "winget".into() };
            }
            settings.save();
            send(webview, json!({ "type": "settings", "settings": settings }));
        }
        "job" => {
            let keys = strings(&msg["keys"]);
            let m = settings.manager();
            let job = match msg["kind"].as_str().unwrap_or_default() {
                "install" => Job::InstallApps(keys, m),
                "uninstall" => Job::UninstallApps(keys, m),
                "upgradeAll" => Job::UpgradeAll(m),
                "detectApps" => Job::DetectApps(m),
                "runTweaks" => Job::RunTweaks(keys),
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
            let _ = jobs.send(job);
        }
        "panel" => {
            if let Err(e) = open_panel(&str_of("key")) {
                send(webview, json!({ "type": "log", "text": e, "kind": "error" }));
            }
        }
        "openUrl" => {
            // Only links that are in WinUtil's own app list, never arbitrary strings.
            let url = str_of("url");
            let known = engine::load(engine::APPLICATIONS)
                .as_object()
                .cloned()
                .is_some_and(|apps| apps.values().any(|a| a["link"].as_str() == Some(url.as_str())));
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

/// The Legacy Windows Panels buttons: each names a control-panel applet, snap-in or
/// shell location. Opened directly, the way WinUtil hands them to Windows.
fn open_panel(key: &str) -> Result<(), String> {
    let features = engine::load(engine::FEATURES);
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
