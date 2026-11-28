use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, IDI_APPLICATION,
    MF_SEPARATOR, MF_STRING, MSG, SW_SHOWNORMAL, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

const TRAY_ICON_ID: u32 = 1;
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const ID_OPEN_CONFIG: usize = 1001;
const ID_OPEN_STATUS: usize = 1002;
const ID_OPEN_PROGRAM_DIR: usize = 1003;
const ID_EXIT: usize = 1004;

struct TrayContext {
    config_path: PathBuf,
    status_path: PathBuf,
    program_dir: PathBuf,
}

static CONTEXT: OnceLock<TrayContext> = OnceLock::new();

pub fn run(config_path: &Path) -> Result<()> {
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("无法解析配置文件路径 {}", config_path.display()))?;
    let executable = std::env::current_exe().context("无法确定程序路径")?;
    let program_dir = executable
        .parent()
        .context("程序路径没有父目录")?
        .to_path_buf();
    CONTEXT
        .set(TrayContext {
            status_path: config_path.with_file_name("proxy-gate-status.json"),
            config_path,
            program_dir,
        })
        .map_err(|_| anyhow::anyhow!("托盘已经启动"))?;

    unsafe { run_message_loop() }
}

unsafe fn run_message_loop() -> Result<()> {
    let class_name = wide_null("ProxyGate.TrayWindow");
    let window_name = wide_null("Proxy Gate");
    let instance = GetModuleHandleW(ptr::null());
    if instance.is_null() {
        bail!("GetModuleHandleW 失败");
    }
    let icon = LoadIconW(ptr::null_mut(), IDI_APPLICATION);
    if icon.is_null() {
        bail!("无法加载托盘图标");
    }

    let window_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: icon,
        hCursor: ptr::null_mut(),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    if RegisterClassW(&window_class) == 0 {
        bail!("注册托盘窗口类失败");
    }

    let window = CreateWindowExW(
        0,
        class_name.as_ptr(),
        window_name.as_ptr(),
        0,
        0,
        0,
        0,
        0,
        ptr::null_mut(),
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    if window.is_null() {
        bail!("创建托盘消息窗口失败");
    }

    let icon_data = notify_icon_data(window, icon);
    if Shell_NotifyIconW(NIM_ADD, &icon_data) == 0 {
        DestroyWindow(window);
        bail!("无法添加系统托盘图标");
    }
    println!("Proxy Gate 托盘已启动，右键托盘图标可打开文件或退出");

    let mut message = MSG::default();
    loop {
        let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
        if result == -1 {
            remove_tray_icon(window);
            DestroyWindow(window);
            bail!("读取托盘消息失败");
        }
        if result == 0 {
            break;
        }
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    remove_tray_icon(window);
    Ok(())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY_CALLBACK => {
            let event = lparam as u32;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                show_menu(window);
            } else if event == WM_LBUTTONDBLCLK {
                if let Some(context) = CONTEXT.get() {
                    let _ = open_path(window, &context.status_path);
                }
            }
            0
        }
        WM_COMMAND => {
            let command = wparam & 0xffff;
            if let Some(context) = CONTEXT.get() {
                match command {
                    ID_OPEN_CONFIG => {
                        let _ = open_path(window, &context.config_path);
                    }
                    ID_OPEN_STATUS => {
                        let _ = open_path(window, &context.status_path);
                    }
                    ID_OPEN_PROGRAM_DIR => {
                        let _ = open_path(window, &context.program_dir);
                    }
                    ID_EXIT => {
                        DestroyWindow(window);
                    }
                    _ => {}
                }
            }
            0
        }
        WM_DESTROY => {
            remove_tray_icon(window);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn show_menu(window: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    append_menu(menu, MF_STRING, ID_OPEN_CONFIG, "打开配置");
    append_menu(menu, MF_STRING, ID_OPEN_STATUS, "打开状态文件");
    append_menu(menu, MF_STRING, ID_OPEN_PROGRAM_DIR, "打开程序目录");
    append_menu(menu, MF_SEPARATOR, 0, "");
    append_menu(menu, MF_STRING, ID_EXIT, "退出");

    let mut point = POINT::default();
    if GetCursorPos(&mut point) != 0 {
        SetForegroundWindow(window);
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            window,
            ptr::null(),
        );
        PostMessageW(window, WM_NULL, 0, 0);
    }
    DestroyMenu(menu);
}

unsafe fn append_menu(menu: *mut std::ffi::c_void, flags: u32, id: usize, label: &str) {
    let label = wide_null(label);
    AppendMenuW(menu, flags, id, label.as_ptr());
}

unsafe fn open_path(window: HWND, path: &Path) -> Result<()> {
    let operation = wide_null("open");
    let path_text = path.display().to_string();
    let path = wide_null(&path_text);
    let result = ShellExecuteW(
        window,
        operation.as_ptr(),
        path.as_ptr(),
        ptr::null(),
        ptr::null(),
        SW_SHOWNORMAL,
    ) as isize;
    if result <= 32 {
        bail!("无法打开 {path_text}");
    }
    Ok(())
}

fn notify_icon_data(window: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: icon,
        ..NOTIFYICONDATAW::default()
    };
    copy_wide(&mut data.szTip, "Proxy Gate");
    data
}

unsafe fn remove_tray_icon(window: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        ..NOTIFYICONDATAW::default()
    };
    Shell_NotifyIconW(NIM_DELETE, &data);
}

fn copy_wide(destination: &mut [u16], value: &str) {
    let mut encoded = value.encode_utf16();
    for slot in destination.iter_mut() {
        *slot = encoded.next().unwrap_or(0);
    }
    if let Some(last) = destination.last_mut() {
        *last = 0;
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
