#![windows_subsystem = "windows"]
//! winproc-gui: a fast, keyboard-driven process monitor for Windows.

mod app;
mod columns;
mod format;
mod ini;
mod keymap;
mod platform;
mod render;
mod sampler;
mod search;
mod settings;
mod textbox;
mod theme;

use app::{App, Cursor, WM_APP_SNAPSHOT};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

/// Not in WindowsAndMessaging (it lives with the common controls).
const WM_MOUSELEAVE: u32 = 0x02A3;

fn app_of(hwnd: HWND) -> Option<&'static mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    unsafe { p.as_mut() }
}

fn xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(app) = app_of(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                app.paint();
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let (w, h) = xy(lparam);
                app.on_size(w as u16 as u32, h as u16 as u32, wparam.0 == SIZE_MINIMIZED as usize);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                app.dpi = (wparam.0 & 0xFFFF) as f32;
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                app.on_size(rc.right as u32, rc.bottom as u32, false);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let mi = &mut *(lparam.0 as *mut MINMAXINFO);
                let s = GetDpiForWindow(hwnd).max(96) as i32;
                mi.ptMinTrackSize = POINT { x: 520 * s / 96, y: 360 * s / 96 };
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                app.suppress_char = false;
                if app.on_key(wparam.0 as u16) {
                    app.suppress_char = true;
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_CHAR => {
                app.on_char(wparam.0 as u16);
                LRESULT(0)
            }
            // An Alt shortcut we handled: no menu beep.
            WM_SYSCHAR if app.suppress_char => {
                app.suppress_char = false;
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = xy(lparam);
                app.on_mouse_down(x, y);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = xy(lparam);
                app.on_mouse_move(x, y);
                LRESULT(0)
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                app.on_mouse_up();
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                app.on_mouse_leave();
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let (x, y) = xy(lparam);
                app.on_right_click(x, y);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let (x, y) = xy(lparam);
                app.on_wheel((wparam.0 >> 16) as u16 as i16, x, y);
                LRESULT(0)
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let id = match app.cursor_at(pt.x, pt.y) {
                    Cursor::Arrow => IDC_ARROW,
                    Cursor::SizeNs => IDC_SIZENS,
                    Cursor::Beam => IDC_IBEAM,
                };
                if let Ok(c) = LoadCursorW(None, id) {
                    SetCursor(Some(c));
                }
                LRESULT(1)
            }
            WM_TIMER => {
                app.on_timer(wparam.0);
                LRESULT(0)
            }
            WM_APP_SNAPSHOT => {
                app.on_snapshot();
                LRESULT(0)
            }
            WM_ACTIVATE => {
                if wparam.0 & 0xFFFF != 0 {
                    app.check_config();
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_SETTINGCHANGE => {
                if lparam.0 != 0 {
                    let s = PCWSTR(lparam.0 as *const u16);
                    if s.to_string().map(|s| s == "ImmersiveColorSet").unwrap_or(false) {
                        app.on_system_theme_change();
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_QUERYENDSESSION => {
                app.save_state();
                LRESULT(1)
            }
            WM_CLOSE => {
                app.shutdown();
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = w!("winproc-gui");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hIcon: LoadIconW(Some(hinstance), PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let s = GetDpiForSystem().max(96) as i32;
        let title = if platform::is_elevated() { w!("winproc-gui (Administrator)") } else { w!("winproc-gui") };
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            title,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1180 * s / 96,
            780 * s / 96,
            None,
            None,
            Some(hinstance),
            None,
        ) else {
            return;
        };
        let dpi = GetDpiForWindow(hwnd).max(96) as f32;
        let app = Box::into_raw(App::new(hwnd, dpi));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);

        let (rect, maximized) = (*app).saved_placement();
        let show = if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL };
        match rect {
            Some((x, y, w, h)) => {
                let wp = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    showCmd: show.0 as u32,
                    rcNormalPosition: RECT { left: x, top: y, right: x + w, bottom: y + h },
                    ..Default::default()
                };
                if SetWindowPlacement(hwnd, &wp).is_err() {
                    let _ = ShowWindow(hwnd, show);
                }
            }
            None => {
                let _ = ShowWindow(hwnd, show);
            }
        }
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        (*app).dpi = GetDpiForWindow(hwnd).max(96) as f32;
        (*app).on_size(rc.right as u32, rc.bottom as u32, false);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(app));
    }
}
