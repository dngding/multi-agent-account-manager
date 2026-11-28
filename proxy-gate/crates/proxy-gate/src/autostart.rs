use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE_NAME: &str = "ProxyGate";
const LAUNCHER_FILE_NAME: &str = "proxy-gate-tray.vbs";

pub fn install(config_path: &Path) -> Result<()> {
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("配置文件不存在 {}", config_path.display()))?;
    let executable = env::current_exe()
        .context("无法确定 proxy-gate.exe 路径")?
        .canonicalize()
        .context("无法解析 proxy-gate.exe 路径")?;
    let launcher = launcher_path()?;
    let launcher_contents = render_launcher(&executable, &config_path)?;
    if let Some(parent) = launcher.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建自启动目录 {}", parent.display()))?;
    }
    write_utf16_with_bom(&launcher, &launcher_contents)
        .with_context(|| format!("无法写入自启动脚本 {}", launcher.display()))?;

    let command = format!(
        "wscript.exe //B //NoLogo \"{}\"",
        launcher.display().to_string().replace('"', "")
    );
    let key = open_run_key()?;
    let result = set_string_value(key, &command);
    unsafe {
        RegCloseKey(key);
    }
    result?;

    println!("已启用登录自启动：HKCU\\{RUN_KEY}\\{RUN_VALUE_NAME}");
    println!("托盘程序：{}", executable.display());
    println!("配置文件：{}", config_path.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let key = open_run_key()?;
    let name = wide_null(RUN_VALUE_NAME);
    let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    unsafe {
        RegCloseKey(key);
    }
    if status != 0 && status != ERROR_FILE_NOT_FOUND {
        bail!("删除登录自启动项失败，Win32 错误 {status}");
    }

    if let Ok(path) = launcher_path() {
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error)
                    .with_context(|| format!("无法删除自启动脚本 {}", path.display()));
            }
        }
    }
    println!("已移除登录自启动项");
    Ok(())
}

fn open_run_key() -> Result<HKEY> {
    let subkey = wide_null(RUN_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if status != 0 {
        bail!("打开 HKCU Run 注册表项失败，Win32 错误 {status}");
    }
    Ok(key)
}

fn set_string_value(key: HKEY, value: &str) -> Result<()> {
    let name = wide_null(RUN_VALUE_NAME);
    let data = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let size = u32::try_from(data.len()).context("自启动命令过长")?;
    let status = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, data.as_ptr(), size) };
    if status != 0 {
        bail!("写入 HKCU Run 注册表项失败，Win32 错误 {status}");
    }
    Ok(())
}

fn launcher_path() -> Result<PathBuf> {
    let local_app_data = env::var_os("LOCALAPPDATA").context("环境变量 LOCALAPPDATA 不存在")?;
    Ok(PathBuf::from(local_app_data)
        .join("ProxyGate")
        .join(LAUNCHER_FILE_NAME))
}

fn render_launcher(executable: &Path, config_path: &Path) -> Result<String> {
    let executable = executable.to_str().context("程序路径不是有效 Unicode")?;
    let config_path = config_path.to_str().context("配置路径不是有效 Unicode")?;
    if executable.contains('"') || config_path.contains('"') {
        bail!("程序或配置路径包含不支持的引号字符");
    }

    Ok(format!(
        "CreateObject(\"WScript.Shell\").Run \"\"\"{}\"\" tray --config \"\"{}\"\"\", 0, False\r\n",
        executable, config_path
    ))
}

fn write_utf16_with_bom(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(2 + contents.len() * 2);
    bytes.extend_from_slice(&[0xff, 0xfe]);
    for unit in contents.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, bytes)
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::render_launcher;

    #[test]
    fn launcher_hides_console_and_uses_absolute_config() {
        let script = render_launcher(
            Path::new(r"C:\Tools\proxy-gate.exe"),
            Path::new(r"C:\Users\A B\proxy-gate.json"),
        )
        .unwrap();
        assert!(script.contains(
            r#""""C:\Tools\proxy-gate.exe"" tray --config ""C:\Users\A B\proxy-gate.json""""#
        ));
        assert!(script.ends_with(", 0, False\r\n"));
    }
}
