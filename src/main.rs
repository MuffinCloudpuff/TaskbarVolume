#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]

//! A deliberately small resident utility.  It installs one low-level mouse hook,
//! and only consumes wheel input while the pointer is over a taskbar window.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use windows::core::{w, BOOL, Result};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_VOLUME_DOWN, VK_VOLUME_UP};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateWindowExW, CreatePopupMenu, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowExW, GetClassNameW, GetWindowRect,
    GetCursorPos, GetMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    SetWindowsHookExW, ShowWindow, TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx,
    CreateIconFromResourceEx, HICON, MENU_ITEM_FLAGS, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, SW_HIDE,
    TPM_RIGHTBUTTON, WH_MOUSE_LL, WM_COMMAND, WM_DESTROY, WM_MOUSEWHEEL, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPED, MB_ICONERROR, MB_OK, MessageBoxW,
};

const TRAY_ICON_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = 0x8001; // WM_APP + 1
const EXIT_COMMAND_ID: usize = 1;
const AUTOSTART_COMMAND_ID: usize = 2;
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: windows::core::PCWSTR = w!("TaskbarVolume");

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

fn main() {
    if let Err(error) = run() {
        let error_text: Vec<u16> = format!("Taskbar Volume could not start:\n\n{error}\0")
            .encode_utf16()
            .collect();
        unsafe {
            let _ = MessageBoxW(None, windows::core::PCWSTR(error_text.as_ptr()), w!("Taskbar Volume"), MB_OK | MB_ICONERROR);
        }
    }
}

fn run() -> Result<()> {
    unsafe {
        initialize_log();
        // Mouse hook coordinates are physical pixels; use the same units for window rectangles.
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        log("starting Taskbar Volume");
        log("using Windows media keys so the native volume flyout is shown");

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let hwnd = create_hidden_window(instance)?;
        add_tray_icon(hwnd)?;
        log("notification-area icon added");

        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0)?;
        log("low-level mouse hook installed; entering message loop");
        message_loop();
        UnhookWindowsHookEx(hook)?;
        remove_tray_icon(hwnd);
    }
    Ok(())
}

fn initialize_log() {
    let Ok(executable) = std::env::current_exe() else { return };
    let log_path = executable.with_file_name("taskbar-volume.log");
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path) else { return };
    let _ = LOG_FILE.set(Mutex::new(file));
}

fn log(message: &str) {
    if let Some(lock) = LOG_FILE.get() {
        if let Ok(mut file) = lock.lock() {
            let _ = writeln!(file, "{message}");
            let _ = file.flush();
        }
    }
}

unsafe fn create_hidden_window(instance: HINSTANCE) -> Result<HWND> {
    let class = w!("TaskbarVolumeHiddenWindow");
    let window_class = WNDCLASSW {
        hInstance: instance,
        lpszClassName: class,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };
    RegisterClassW(&window_class);
    let hwnd = CreateWindowExW(
        Default::default(), class, w!("Taskbar Volume"), WS_OVERLAPPED,
        0, 0, 0, 0, None, None, Some(instance), None,
    )?;
    let _ = ShowWindow(hwnd, SW_HIDE);
    Ok(hwnd)
}

unsafe fn add_tray_icon(hwnd: HWND) -> Result<()> {
    let mut icon = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK_MESSAGE,
        hIcon: embedded_tray_icon()?,
        ..Default::default()
    };
    let label: Vec<u16> = "Taskbar Volume — wheel over taskbar to change volume\0".encode_utf16().collect();
    icon.szTip[..label.len()].copy_from_slice(&label);
    Shell_NotifyIconW(NIM_ADD, &icon).ok()
}

/// Builds an HICON from the ICO stored inside this executable at compile time.
/// Keeping it embedded means the tray image also works after the EXE is moved.
unsafe fn embedded_tray_icon() -> Result<HICON> {
    const ICON: &[u8] = include_bytes!("../assets/taskbar-volume.ico");
    // ICO header is 6 bytes; its first directory entry stores byte length at
    // 14..18 and image offset at 18..22. The generated asset has one entry.
    let bytes_in_resource = u32::from_le_bytes(ICON[14..18].try_into().expect("valid ICO size")) as usize;
    let image_offset = u32::from_le_bytes(ICON[18..22].try_into().expect("valid ICO offset")) as usize;
    let image = ICON
        .get(image_offset..image_offset + bytes_in_resource)
        .expect("valid ICO image data");
    CreateIconFromResourceEx(image, true, 0x0003_0000, 0, 0, Default::default())
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let icon = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
}

unsafe fn message_loop() {
    let mut message = MSG::default();
    while GetMessageW(&mut message, None, 0, 0).into() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    if message == TRAY_CALLBACK_MESSAGE && lparam.0 as u32 == WM_RBUTTONUP {
        show_tray_menu(hwnd);
        return LRESULT(0);
    }
    if message == WM_COMMAND && (wparam.0 & 0xffff) == EXIT_COMMAND_ID {
        let _ = DestroyWindow(hwnd);
        return LRESULT(0);
    }
    if message == WM_COMMAND && (wparam.0 & 0xffff) == AUTOSTART_COMMAND_ID {
        if let Err(error) = set_autostart(!autostart_enabled()) {
            log(&format!("could not update autostart: {error}"));
        }
        return LRESULT(0);
    }
    if message == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().expect("failed to create tray menu");
    let state = if autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(MF_STRING.0 | state.0), AUTOSTART_COMMAND_ID, w!("Start with Windows"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, EXIT_COMMAND_ID, w!("Exit"));
    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, Some(0), hwnd, None);
    let _ = DestroyMenu(menu);
}

unsafe fn autostart_enabled() -> bool {
    let mut key = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_QUERY_VALUE, &mut key).is_err() {
        return false;
    }
    let mut size = 0;
    let exists = RegQueryValueExW(key, RUN_VALUE, None, None, None, Some(&mut size)).is_ok();
    let _ = RegCloseKey(key);
    exists
}

unsafe fn set_autostart(enabled: bool) -> Result<()> {
    let mut key = HKEY::default();
    RegCreateKeyExW(
        HKEY_CURRENT_USER, RUN_KEY, None, None, REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE, None, &mut key, None,
    ).ok()?;
    let result = if enabled {
        let executable = std::env::current_exe().expect("executable path must be available");
        let command: Vec<u16> = format!("\"{}\"", executable.display()).encode_utf16().chain(Some(0)).collect();
        let bytes = std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2);
        RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)).ok()
    } else {
        RegDeleteValueW(key, RUN_VALUE).ok()
    };
    let _ = RegCloseKey(key);
    result
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEWHEEL {
        let hook_data = *(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::MSLLHOOKSTRUCT);
        let (on_taskbar, class_name) = cursor_taskbar_state(hook_data.pt);
        log(&format!("wheel received: taskbar={on_taskbar}, target={class_name}"));
        if !on_taskbar {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let delta = ((hook_data.mouseData >> 16) as i16) as f32;
        if delta != 0.0 {
            let key = if delta > 0.0 { VK_VOLUME_UP } else { VK_VOLUME_DOWN };
            keybd_event(key.0 as u8, 0, Default::default(), 0);
            keybd_event(key.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            log(if delta > 0.0 { "sent VK_VOLUME_UP" } else { "sent VK_VOLUME_DOWN" });
            // Do not let taskbar controls also receive this wheel action.
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

struct TaskbarHit {
    point: POINT,
    found: bool,
}

unsafe fn cursor_taskbar_state(point: POINT) -> (bool, String) {
    let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let mut hit = TaskbarHit { point, found: false };
    for class in [w!("Shell_TrayWnd"), w!("Shell_SecondaryTrayWnd")] {
        let mut after = None;
        while let Ok(window) = FindWindowExW(None, after, class, None) {
            if window.0.is_null() { break; }
            let _ = check_taskbar(window, LPARAM((&mut hit as *mut TaskbarHit) as isize));
            after = Some(window);
        }
    }
    if !previous_dpi.0.is_null() {
        let _ = SetThreadDpiAwarenessContext(previous_dpi);
    }
    (hit.found, format!("taskbar region at {},{}", point.x, point.y))
}

unsafe extern "system" fn check_taskbar(window: HWND, parameter: LPARAM) -> BOOL {
    let hit = &mut *(parameter.0 as *mut TaskbarHit);
    let mut class_name = [0u16; 128];
    let copied = GetClassNameW(window, &mut class_name);
    let name = String::from_utf16_lossy(&class_name[..copied as usize]);
    if name != "Shell_TrayWnd" && name != "Shell_SecondaryTrayWnd" {
        return BOOL(1);
    }
    let mut rect = RECT::default();
    if GetWindowRect(window, &mut rect).is_err() {
        return BOOL(1);
    }
    let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut monitor).as_bool() {
        // Extend a bottom taskbar through the last pixel of its own monitor.
        // Restrict X as well, so a taskbar on another monitor cannot match.
        if rect.bottom >= monitor.rcMonitor.bottom - 2 && rect.top < monitor.rcMonitor.bottom {
            rect.bottom = monitor.rcMonitor.bottom;
            rect.left = monitor.rcMonitor.left;
            rect.right = monitor.rcMonitor.right;
        }
    }
    if contains_point(rect, hit.point) {
        hit.found = true;
    }
    log(&format!("taskbar rect={},{},{},{} point={},{} hit={}", rect.left, rect.top, rect.right, rect.bottom, hit.point.x, hit.point.y, hit.found));
    BOOL(1)
}

fn contains_point(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a live Windows Explorer taskbar"]
    fn live_taskbar_center_is_detected() {
        unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let window = FindWindowExW(None, None, w!("Shell_TrayWnd"), None).unwrap();
            assert!(!window.0.is_null());
            let mut rect = RECT::default();
            GetWindowRect(window, &mut rect).unwrap();
            let point = POINT { x: (rect.left + rect.right) / 2, y: rect.bottom - 1 };
            assert!(cursor_taskbar_state(point).0, "taskbar bottom pixel was not detected");
        }
    }

    #[test]
    fn bottom_edge_and_taskbar_top_are_included() {
        let rect = RECT { left: -1920, top: 1032, right: 0, bottom: 1080 };
        assert!(contains_point(rect, POINT { x: -1, y: 1079 }));
        assert!(contains_point(rect, POINT { x: -1920, y: 1032 }));
        assert!(!contains_point(rect, POINT { x: -1, y: 1031 }));
        assert!(!contains_point(rect, POINT { x: 0, y: 1079 }));
        assert!(!contains_point(rect, POINT { x: -1, y: 1080 }));
    }
}
