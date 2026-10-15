//! The Windows plumbing: registry, services, child processes, elevation, and the
//! named events that keep one copy running and let other programs ask it to quit.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE,
    REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_QWORD, REG_SZ,
};
use windows_sys::Win32::System::Threading::{CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, INFINITE};
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// No console window for the tools RustWinUtil runs in the background.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ------------------------------------------------------------------- registry

/// A registry value as read back, enough to compare against config strings.
#[derive(Debug, Clone, PartialEq)]
pub enum RegValue {
    Dword(u32),
    Qword(u64),
    Text(String),
    Other,
}

impl RegValue {
    /// Whether this equals a value written as text in WinUtil's config. DWORDs
    /// compare both as unsigned and signed, since PowerShell reads them signed.
    pub fn matches(&self, expected: &str) -> bool {
        let expected = expected.trim();
        match self {
            RegValue::Dword(v) => {
                expected.parse::<u32>().ok() == Some(*v) || expected.parse::<i32>().ok() == Some(*v as i32)
            }
            RegValue::Qword(v) => {
                expected.parse::<u64>().ok() == Some(*v) || expected.parse::<i64>().ok() == Some(*v as i64)
            }
            RegValue::Text(s) => s == expected,
            RegValue::Other => false,
        }
    }
}

/// Splits "HKLM:\SOFTWARE\..." (PowerShell drive notation, as WinUtil writes it)
/// into a root key and subkey.
fn split_path(path: &str) -> Result<(HKEY, String), String> {
    let (root, rest) = path.split_once(":\\").or_else(|| path.split_once(':')).ok_or("not a registry path")?;
    let root = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        "HKU" | "HKEY_USERS" => HKEY_USERS,
        "HKCR" | "HKEY_CLASSES_ROOT" => HKEY_CLASSES_ROOT,
        other => return Err(format!("unknown registry root {other}")),
    };
    Ok((root, rest.trim_start_matches('\\').to_string()))
}

pub fn reg_read(path: &str, name: &str) -> Option<RegValue> {
    let (root, sub) = split_path(path).ok()?;
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(&sub).as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) != 0 {
            return None;
        }
        let name_w = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        let mut result = None;
        if RegQueryValueExW(key, name_w.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut len) == 0 {
            let mut buf = vec![0u8; len as usize + 2];
            if RegQueryValueExW(key, name_w.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr(), &mut len) == 0 {
                buf.truncate(len as usize);
                result = Some(match kind {
                    REG_DWORD if buf.len() >= 4 => RegValue::Dword(u32::from_le_bytes(buf[..4].try_into().unwrap())),
                    REG_QWORD if buf.len() >= 8 => RegValue::Qword(u64::from_le_bytes(buf[..8].try_into().unwrap())),
                    REG_SZ | REG_EXPAND_SZ => {
                        let units: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                        RegValue::Text(String::from_utf16_lossy(&units[..end]))
                    }
                    _ => RegValue::Other,
                });
            }
        }
        RegCloseKey(key);
        result
    }
}

/// Sets a value, creating the key as needed. `"<RemoveEntry>"` deletes it, as in
/// WinUtil's own Set-WinUtilRegistry.
pub fn reg_set(path: &str, name: &str, kind: &str, value: &str) -> Result<(), String> {
    let (root, sub) = split_path(path)?;
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let rc = RegCreateKeyExW(
            root,
            wide(&sub).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE | KEY_READ | KEY_WOW64_64KEY,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(format!("couldn't open {path} (error {rc})"));
        }
        let name_w = wide(name);
        let rc = if value == "<RemoveEntry>" {
            match RegDeleteValueW(key, name_w.as_ptr()) {
                ERROR_FILE_NOT_FOUND => 0, // already gone
                rc => rc,
            }
        } else {
            let bytes: Vec<u8> = match kind.to_ascii_lowercase().as_str() {
                "dword" => value
                    .trim()
                    .parse::<u32>()
                    .or_else(|_| value.trim().parse::<i32>().map(|v| v as u32))
                    .map_err(|_| format!("{value} isn't a DWORD"))?
                    .to_le_bytes()
                    .to_vec(),
                "qword" => value
                    .trim()
                    .parse::<u64>()
                    .or_else(|_| value.trim().parse::<i64>().map(|v| v as u64))
                    .map_err(|_| format!("{value} isn't a QWORD"))?
                    .to_le_bytes()
                    .to_vec(),
                _ => wide(value).iter().flat_map(|u| u.to_le_bytes()).collect(),
            };
            let reg_kind = match kind.to_ascii_lowercase().as_str() {
                "dword" => REG_DWORD,
                "qword" => REG_QWORD,
                "expandstring" => REG_EXPAND_SZ,
                _ => REG_SZ,
            };
            RegSetValueExW(key, name_w.as_ptr(), 0, reg_kind, bytes.as_ptr(), bytes.len() as u32)
        };
        RegCloseKey(key);
        if rc != 0 {
            return Err(format!("couldn't write {path}\\{name} (error {rc})"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- services

/// A service's startup type in PowerShell's words: Automatic,
/// AutomaticDelayedStart, Manual, Disabled, Boot or System. None if no such service.
pub fn service_start_type(name: &str) -> Option<String> {
    let key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{name}");
    let start = match reg_read(&key, "Start")? {
        RegValue::Dword(v) => v,
        _ => return None,
    };
    let delayed = matches!(reg_read(&key, "DelayedAutostart"), Some(RegValue::Dword(1)));
    Some(
        match (start, delayed) {
            (0, _) => "Boot",
            (1, _) => "System",
            (2, true) => "AutomaticDelayedStart",
            (2, false) => "Automatic",
            (3, _) => "Manual",
            (4, _) => "Disabled",
            _ => "Unknown",
        }
        .to_string(),
    )
}

/// Whether a startup type read with [`service_start_type`] counts as `wanted`.
/// Windows PowerShell reports delayed-start services as plain Automatic.
pub fn start_type_is(actual: &str, wanted: &str) -> bool {
    actual.eq_ignore_ascii_case(wanted) || (actual == "AutomaticDelayedStart" && wanted.eq_ignore_ascii_case("Automatic"))
}

pub fn service_set(name: &str, startup: &str, log: &dyn Fn(String)) -> Result<(), String> {
    let mode = match startup.to_ascii_lowercase().as_str() {
        "automatic" => "auto",
        "automaticdelayedstart" => "delayed-auto",
        "manual" => "demand",
        "disabled" => "disabled",
        other => return Err(format!("unknown startup type {other}")),
    };
    let code = run(Command::new("sc.exe").args(["config", name, "start=", mode]), &|_| {})?;
    if code != 0 {
        return Err(format!("sc.exe couldn't change {name} (exit code {code})"));
    }
    log(format!("Service {name} set to {startup}"));
    Ok(())
}

// ------------------------------------------------------------------ processes

/// Runs a program without a window, passing each line of its output (both
/// streams) to `line` as it arrives. Returns the exit code.
pub fn run(cmd: &mut Command, line: &dyn Fn(String)) -> Result<i32, String> {
    let program = cmd.get_program().to_string_lossy().to_string();
    let mut child = cmd
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {program} ({e})"))?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || pump(stream, tx)));
    }
    drop(tx);
    for text in rx {
        line(text);
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| format!("{program} didn't finish ({e})"))?;
    Ok(status.code().unwrap_or(-1))
}

/// Splits a byte stream into lines on \n and \r, dropping blank lines and the
/// spinner and progress-bar frames that tools like winget redraw in place.
fn pump(mut stream: Box<dyn Read + Send>, tx: mpsc::Sender<String>) {
    let mut buf = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    let flush = |bytes: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(bytes).trim().to_string();
        bytes.clear();
        let spinner = text.len() <= 2 && text.chars().all(|c| "-\\|/".contains(c));
        let bar = text.contains('█') || text.contains('▒');
        if !text.is_empty() && !spinner && !bar {
            let _ = tx.send(text);
        }
    };
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                flush(&mut pending);
            } else {
                pending.push(b);
            }
        }
    }
    flush(&mut pending);
}

/// Opens a file, program or shell location the way Explorer would.
pub fn shell_open(target: &str, args: Option<&str>) -> Result<(), String> {
    let (verb, file) = (wide("open"), wide(target));
    let params = args.map(wide);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize > 32 {
        Ok(())
    } else {
        Err(format!("Windows couldn't open {target}"))
    }
}

// ------------------------------------------------------------------ elevation

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Starts this exe again as administrator. False if the prompt was declined.
pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let args: Vec<String> = std::env::args().skip(1).map(|a| format!("\"{a}\"")).collect();
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(&args.join(" ")));
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL)
    };
    result as usize > 32
}

// ------------------------------------------------------------- one instance

/// SYSTEM and Administrators: everything. Authenticated users: set and wait
/// (EVENT_MODIFY_STATE | SYNCHRONIZE). Medium mandatory label, so an unelevated
/// program, such as the Javacrafter Launcher, can ring this elevated app's events.
const EVENT_DESCRIPTOR: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100002;;;AU)S:(ML;;NW;;;ME)";
const EVENT_MODIFY_STATE: u32 = 0x0002;

/// Named per install folder, so copies kept in different folders don't meet.
fn event_name(which: &str) -> String {
    let folder = crate::app_dir().to_string_lossy().to_lowercase();
    let hash = folder.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("Local\\RustWinUtil-{hash:016x}-{which}")
}

/// Rings the running copy's event. False when no copy is running.
pub fn signal(which: &str) -> bool {
    unsafe {
        let handle = OpenEventW(EVENT_MODIFY_STATE, 0, wide(&event_name(which)).as_ptr());
        if handle.is_null() {
            return false;
        }
        SetEvent(handle);
        CloseHandle(handle);
        true
    }
}

/// This copy's "Show" and "Quit" events. None when another copy owns them.
pub struct Instance {
    show: HANDLE,
    quit: HANDLE,
}

// The handles are only waited on and closed by the process; safe to move.
unsafe impl Send for Instance {}

pub fn claim() -> Option<Instance> {
    unsafe {
        let mut sd = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(EVENT_DESCRIPTOR).as_ptr(), 1, &mut sd, std::ptr::null_mut()) == 0 {
            return None;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let show = CreateEventW(&sa, 0, 0, wide(&event_name("Show")).as_ptr());
        let taken = GetLastError() == ERROR_ALREADY_EXISTS;
        let quit = CreateEventW(&sa, 0, 0, wide(&event_name("Quit")).as_ptr());
        LocalFree(sd as _);
        if show.is_null() || quit.is_null() || taken {
            return None;
        }
        Some(Instance { show, quit })
    }
}

impl Instance {
    /// Waits for the events forever, calling `on_show` or `on_quit` as they ring.
    pub fn listen(self, on_show: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) {
        let me = self;
        std::thread::spawn(move || loop {
            // Taken whole, so the thread owns the (Send) Instance rather than its raw handles.
            let me = &me;
            let handles = [me.show, me.quit];
            match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
                0 => on_show(),
                1 => {
                    on_quit();
                    return;
                }
                _ => return,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes, reads back and removes values under a throwaway key of its own.
    #[test]
    fn registry_round_trip() {
        let key = r"HKCU:\Software\RustWinUtilTest";
        reg_set(key, "Dword", "DWord", "4294967295").unwrap();
        reg_set(key, "Qword", "QWord", "12345678901").unwrap();
        reg_set(key, "Text", "String", "hello").unwrap();
        assert!(reg_read(key, "Dword").unwrap().matches("4294967295"));
        assert!(reg_read(key, "Dword").unwrap().matches("-1"), "DWORDs also compare as PowerShell's signed reading");
        assert!(reg_read(key, "Qword").unwrap().matches("12345678901"));
        assert!(reg_read(key, "Text").unwrap().matches("hello"));
        reg_set(key, "Text", "String", "<RemoveEntry>").unwrap();
        assert_eq!(reg_read(key, "Text"), None);
        reg_set(key, "Text", "String", "<RemoveEntry>").unwrap(); // already gone is fine
        assert_eq!(service_start_type("RustWinUtilNoSuchService"), None);
        assert!(service_start_type("Winmgmt").is_some());
        // Tidy up the throwaway key.
        let _ = Command::new("reg.exe").args(["delete", r"HKCU\Software\RustWinUtilTest", "/f"]).output();
    }
}
