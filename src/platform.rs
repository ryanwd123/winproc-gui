//! Windows calls that are not drawing or sampling: ending processes, priorities,
//! the clipboard, the shell, elevation and the title bar.

use std::path::Path;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

/// One process, named by PID and start time so a reused PID is never hit.
#[derive(Clone, Debug)]
pub struct Target {
    pub pid: u32,
    pub create_time: u64,
    pub name: String,
}

/// Ending these takes Windows down (or Windows refuses anyway).
const CRITICAL: &[&str] = &[
    "system", "system idle process", "registry", "secure system", "memory compression",
    "smss.exe", "csrss.exe", "wininit.exe", "services.exe", "lsass.exe", "winlogon.exe",
    "lsaiso.exe", "fontdrvhost.exe",
];

pub fn is_protected(name: &str, pid: u32) -> bool {
    pid == 0 || pid == 4 || CRITICAL.contains(&name.to_lowercase().as_str())
}

fn error_text(e: &windows::core::Error) -> String {
    if e.code() == E_ACCESSDENIED {
        if is_elevated() {
            "access denied".into()
        } else {
            "access denied (try Restart as administrator)".into()
        }
    } else {
        e.message().trim().to_string()
    }
}

fn open_checked(t: &Target, access: PROCESS_ACCESS_RIGHTS) -> Result<HANDLE, String> {
    let h = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, t.pid) }
        .map_err(|e| error_text(&e))?;
    let (mut c, mut x, mut k, mut u) = Default::default();
    if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) }.is_ok() {
        let created = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
        if t.create_time != 0 && created != t.create_time {
            unsafe {
                let _ = CloseHandle(h);
            }
            return Err("it already exited".into());
        }
    }
    Ok(h)
}

pub fn kill(t: &Target) -> Result<(), String> {
    if is_protected(&t.name, t.pid) {
        return Err("it is a critical system process".into());
    }
    if t.pid == std::process::id() {
        return Err("that is this app (use Quit)".into());
    }
    let h = open_checked(t, PROCESS_TERMINATE)?;
    unsafe {
        let mut critical = BOOL(0);
        if IsProcessCritical(h, &mut critical).is_ok() && critical.as_bool() {
            let _ = CloseHandle(h);
            return Err("it is a critical system process".into());
        }
        let r = TerminateProcess(h, 1).map_err(|e| error_text(&e));
        let _ = CloseHandle(h);
        r
    }
}

pub fn set_priority(t: &Target, level: u8) -> Result<(), String> {
    let class = match level {
        0 => IDLE_PRIORITY_CLASS,
        1 => BELOW_NORMAL_PRIORITY_CLASS,
        2 => NORMAL_PRIORITY_CLASS,
        3 => ABOVE_NORMAL_PRIORITY_CLASS,
        _ => HIGH_PRIORITY_CLASS,
    };
    let h = open_checked(t, PROCESS_SET_INFORMATION)?;
    unsafe {
        let r = SetPriorityClass(h, class).map_err(|e| error_text(&e));
        let _ = CloseHandle(h);
        r
    }
}

pub fn copy_text(hwnd: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let mut ok = false;
        if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(hg) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(hg);
                ok = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hg.0))).is_ok();
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

pub fn paste_text(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let mut out = None;
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(hg);
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Opens a file with its default program (Notepad when .ini has none).
pub fn open_file(path: &Path) {
    let p = HSTRING::from(path.as_os_str());
    unsafe {
        let r = ShellExecuteW(None, w!("open"), &p, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if r.0 as isize <= 32 {
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), &p, PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

pub fn open_folder(path: &Path) {
    let p = HSTRING::from(path.as_os_str());
    unsafe {
        ShellExecuteW(None, w!("explore"), &p, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Opens Explorer with the file selected.
pub fn reveal(path: &str) {
    let args = HSTRING::from(format!("/select,\"{path}\""));
    unsafe {
        ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn is_elevated() -> bool {
    use std::sync::OnceLock;
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    })
}

/// Starts this exe elevated. False when the user declined the UAC prompt.
pub fn restart_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = HSTRING::from(exe.as_os_str());
    let r = unsafe { ShellExecuteW(None, w!("runas"), &exe, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Dark or light title bar, colored to match the window on Windows 11.
pub fn style_titlebar(hwnd: HWND, dark: bool, caption: u32, text: u32) {
    // COLORREF is 0x00BBGGRR; theme colors are 0xRRGGBBAA.
    let cref = |c: u32| ((c >> 24) & 0xff) | (((c >> 16) & 0xff) << 8) | (((c >> 8) & 0xff) << 16);
    unsafe {
        let v = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as _, 4);
        let c = cref(caption);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &c as *const u32 as _, 4);
        let t = cref(text);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &t as *const u32 as _, 4);
    }
}

pub fn set_topmost(hwnd: HWND, on: bool) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}
