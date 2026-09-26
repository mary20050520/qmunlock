use super::{CredentialStatus, Credentials, Error, Result};

pub fn status() -> CredentialStatus {
    #[cfg(target_os = "macos")]
    {
        macos::status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        match load() {
            Ok(credentials) => ready_status(credentials),
            Err(error) => unavailable_status("unavailable", error.to_string()),
        }
    }
}

fn ready_status(credentials: Credentials) -> CredentialStatus {
    CredentialStatus {
        available: true,
        state: "ready".into(),
        platform: platform_name().into(),
        account_hint: Some(mask_uin(&credentials.uin)),
        message: "已读取 QQ 音乐当前登录信息；解密时会再向 QQ 音乐请求对应文件的 ekey。".into(),
    }
}

fn unavailable_status(state: &str, message: impl Into<String>) -> CredentialStatus {
    CredentialStatus {
        available: false,
        state: state.into(),
        platform: platform_name().into(),
        account_hint: None,
        message: message.into(),
    }
}

pub fn load() -> Result<Credentials> {
    #[cfg(target_os = "macos")]
    {
        macos::load()
    }
    #[cfg(target_os = "windows")]
    {
        windows::load()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(Error::from("当前平台不支持自动读取 QQ 音乐凭据"))
    }
}

pub fn api_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "27"
    } else {
        "20"
    }
}

/// macOS 不提供应用自行取得「完全磁盘访问权限」的 API；这里只能打开准确的
/// 系统设置页面，由用户打开开关。访问受保护的 QQ 音乐容器前会先检测并引导。
pub fn open_full_disk_access_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::open_full_disk_access_settings()
    }
    #[cfg(not(target_os = "macos"))]
    Err(Error::from("完全磁盘访问权限仅适用于 macOS"))
}
fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "未知平台"
    }
}
fn mask_uin(value: &str) -> String {
    if value.len() < 5 {
        "已登录".into()
    } else {
        format!("{}***{}", &value[..2], &value[value.len() - 2..])
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use plist::Value;
    use std::fs::File;
    use std::io::Cursor;
    use std::path::Path;
    use std::process::Command;

    fn login_info_path() -> Result<std::path::PathBuf> {
        Ok(dirs::home_dir()
            .ok_or("找不到用户目录")?
            .join("Library/Containers/com.tencent.QQMusicMac/Data/Library/Preferences/com.tencent.QQMusicMac.plist"))
    }

    pub fn status() -> CredentialStatus {
        let path = match login_info_path() {
            Ok(path) => path,
            Err(error) => return unavailable_status("path_unavailable", error.to_string()),
        };
        match File::open(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                unavailable_status(
                    "permission_required",
                    "未获「完全磁盘访问权限」，无法读取 QQ 音乐最新登录信息。已打开系统设置：请允许 QM Unlock 后回到此处重新检测。",
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => unavailable_status(
                "qqmusic_not_found",
                "未找到 QQ 音乐的登录文件。请安装并登录 QQ 音乐后重新检测。",
            ),
            Err(error) => unavailable_status(
                "read_failed",
                format!("无法访问 QQ 音乐登录文件：{error}"),
            ),
            Ok(_) => match load_from_file(&path) {
                Ok(credentials) => ready_status(credentials),
                Err(error) => unavailable_status("login_unavailable", error.to_string()),
            },
        }
    }

    pub fn load() -> Result<Credentials> {
        let path = login_info_path()?;
        load_from_file(&path).map_err(|error| {
            Error::from(format!(
                "无法读取 QQ 音乐最新登录信息：{error}。请在「系统设置 → 隐私与安全性 → 完全磁盘访问权限」中允许 QM Unlock。"
            ))
        })
    }

    pub fn open_full_disk_access_settings() -> Result<()> {
        let status = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .status()
            .map_err(|error| Error::from(format!("无法打开系统设置：{error}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::from("无法打开「完全磁盘访问权限」设置页面"))
        }
    }

    fn load_from_file(path: &Path) -> Result<Credentials> {
        let outer = Value::from_file(path).map_err(|e| Error::from(format!("读取失败: {e}")))?;
        parse_credentials(&outer)
    }

    fn parse_credentials(outer: &Value) -> Result<Credentials> {
        let archived = outer
            .as_dictionary()
            .and_then(|dict| dict.get("AutoLoginUserInfo"))
            .and_then(Value::as_data)
            .ok_or("QQ 音乐未登录或 plist 中没有 AutoLoginUserInfo")?;
        let inner = Value::from_reader(Cursor::new(archived))
            .map_err(|e| Error::from(format!("无法解析 QQ 音乐登录信息: {e}")))?;
        let objects = inner
            .as_dictionary()
            .and_then(|dict| dict.get("$objects"))
            .and_then(Value::as_array)
            .ok_or("QQ 音乐登录信息格式不受支持")?;
        for object in objects {
            let Some(dict) = object.as_dictionary() else {
                continue;
            };
            if !dict.contains_key("strAuthst") {
                continue;
            }
            let authst = resolve_string(dict.get("strAuthst"), objects).unwrap_or_default();
            let uin = resolve_string(
                dict.get("nUserId").or_else(|| dict.get("strUserAccount")),
                objects,
            )
            .unwrap_or_default();
            let login_type =
                resolve_string(dict.get("loginType"), objects).unwrap_or_else(|| "3".into());
            if !authst.is_empty() && !uin.is_empty() {
                return Ok(Credentials {
                    uin,
                    authst,
                    login_type,
                });
            }
        }
        Err(Error::from("QQ 音乐登录信息中没有有效 authst"))
    }

    fn resolve_string(value: Option<&Value>, objects: &[Value]) -> Option<String> {
        let value = value?;
        if let Some(uid) = value.as_uid() {
            return objects
                .get(uid.get() as usize)
                .and_then(|item| resolve_string(Some(item), objects));
        }
        if let Some(value) = value.as_string() {
            return Some(value.to_owned());
        }
        if let Some(value) = value.as_signed_integer() {
            return Some(value.to_string());
        }
        if let Some(value) = value.as_unsigned_integer() {
            return Some(value.to_string());
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_login_archive() {
            let mut login = plist::Dictionary::new();
            login.insert("strAuthst".into(), Value::String("test-auth".into()));
            login.insert("nUserId".into(), Value::Integer(12345.into()));
            login.insert("loginType".into(), Value::String("3".into()));
            let mut archive = plist::Dictionary::new();
            archive.insert(
                "$objects".into(),
                Value::Array(vec![Value::Dictionary(login)]),
            );
            let mut bytes = Vec::new();
            Value::Dictionary(archive)
                .to_writer_binary(&mut bytes)
                .unwrap();
            let mut outer = plist::Dictionary::new();
            outer.insert("AutoLoginUserInfo".into(), Value::Data(bytes));

            let credentials = parse_credentials(&Value::Dictionary(outer)).unwrap();
            assert_eq!(credentials.uin, "12345");
            assert_eq!(credentials.authst, "test-auth");
            assert_eq!(credentials.login_type, "3");
        }

        #[test]
        fn parses_selected_local_plist() {
            let Ok(path) = std::env::var("QMUNLOCK_TEST_LOGIN_PLIST") else {
                return;
            };
            let credentials = load_from_file(Path::new(&path)).unwrap();
            assert!(!credentials.uin.is_empty());
            assert!(!credentials.authst.is_empty());
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use ::windows::Win32::Foundation::CloseHandle;
    use ::windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use ::windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use ::windows::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    use ::windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    use std::fs;

    pub fn load() -> Result<Credentials> {
        let uin = read_uin()?;
        let processes = find_qqmusic_processes()?;
        let mut failures = Vec::new();
        let mut authst = None;
        for (pid, name) in &processes {
            match scan_authst(*pid) {
                Ok(value) => {
                    authst = Some(value);
                    break;
                }
                Err(error) => failures.push(format!("{name} (PID {pid}): {error}")),
            }
        }
        let authst = authst.ok_or_else(|| {
            Error::from(format!(
                "已发现 QQ 音乐进程（{}），但未读取到 authst。请确认 QQ 音乐已登录；若以管理员身份运行 QQ 音乐，也请以管理员身份运行 QM Unlock。{}",
                processes
                    .iter()
                    .map(|(pid, name)| format!("{name} (PID {pid})"))
                    .collect::<Vec<_>>()
                    .join("、"),
                failures
                    .first()
                    .map(|failure| format!(" 详情：{failure}"))
                    .unwrap_or_default()
            ))
        })?;
        Ok(Credentials {
            uin,
            authst,
            login_type: "3".into(),
        })
    }

    fn read_uin() -> Result<String> {
        let appdata = std::env::var_os("APPDATA").ok_or("APPDATA 不存在")?;
        let config =
            std::path::PathBuf::from(appdata).join("Tencent/QQMusic/QQMusicServiceConfig.ini");
        let content = fs::read_to_string(&config)
            .map_err(|_| Error::from("未找到 QQMusicServiceConfig.ini，请先登录 QQ 音乐"))?;
        content
            .lines()
            .find_map(|line| line.trim().strip_prefix("Uin="))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::from("QQMusicServiceConfig.ini 中没有 Uin"))
    }

    fn find_qqmusic_processes() -> Result<Vec<(u32, String)>> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .map_err(|_| Error::from("无法枚举 QQ 音乐进程"))?;
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = Vec::new();
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let end = entry
                        .szExeFile
                        .iter()
                        .position(|x| *x == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name =
                        String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase();
                    if is_qqmusic_process(&name) {
                        found.push((entry.th32ProcessID, name));
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
            if found.is_empty() {
                Err(Error::from(
                    "没有发现运行中的 QQ 音乐进程。请启动桌面版 QQ 音乐并完成登录后重试",
                ))
            } else {
                Ok(found)
            }
        }
    }

    fn is_qqmusic_process(name: &str) -> bool {
        matches!(
            name,
            "qqmusic.exe"
                | "qqmusicdesktop.exe"
                | "qqmusicservice.exe"
                | "qqmusiccloud.exe"
                | "qqmusichelper.exe"
        ) || name.starts_with("qqmusic")
    }

    fn scan_authst(pid: u32) -> Result<String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
                .map_err(|_| {
                    Error::from("无法读取 QQMusic.exe 内存；请以管理员身份启动本工具后重试")
                })?;
            let result = (|| {
                let mut address = 0usize;
                let mut carry = Vec::new();
                while address < usize::MAX - 0x10000 {
                    let mut info = MEMORY_BASIC_INFORMATION::default();
                    let size = VirtualQueryEx(
                        process,
                        Some(address as *const _),
                        &mut info,
                        std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                    );
                    if size == 0 {
                        break;
                    }
                    let next = (info.BaseAddress as usize).saturating_add(info.RegionSize);
                    if info.State == MEM_COMMIT && info.RegionSize > 0 {
                        let mut offset = 0usize;
                        while offset < info.RegionSize {
                            let count = (info.RegionSize - offset).min(1024 * 1024);
                            let mut buffer = vec![0u8; count];
                            let mut read = 0usize;
                            if ReadProcessMemory(
                                process,
                                (info.BaseAddress as usize + offset) as *const _,
                                buffer.as_mut_ptr() as _,
                                count,
                                Some(&mut read),
                            )
                            .is_ok()
                                && read > 0
                            {
                                buffer.truncate(read);
                                let mut joined = carry.clone();
                                joined.extend_from_slice(&buffer);
                                if let Some(token) = extract_authst(&joined) {
                                    return Ok(token);
                                }
                                carry = joined.split_off(joined.len().saturating_sub(2048));
                            }
                            offset = offset.saturating_add(count);
                        }
                    }
                    if next <= address {
                        break;
                    }
                    address = next;
                }
                Err(Error::from(
                    "未在 QQ 音乐内存中找到可用 authst；当前 QQ 音乐版本可能没有将登录令牌以可读取文本保留在进程内存中",
                ))
            })();
            let _ = CloseHandle(process);
            result
        }
    }

    fn extract_authst(data: &[u8]) -> Option<String> {
        // QQ Music has used plain JSON, escaped JSON, form data, and UTF-16
        // request strings in different desktop releases.
        for key in [b"\"authst\"".as_slice(), b"\\\"authst\\\"", b"authst="] {
            for start in find_all(data, key) {
                if let Some(value) = token_after(&data[start + key.len()..]) {
                    return Some(value);
                }
            }
        }

        let utf16_key = b"a\0u\0t\0h\0s\0t\0";
        for start in find_all(data, utf16_key) {
            let (pairs, _) = data[start..].as_chunks::<2>();
            let units = pairs
                .iter()
                .take(1024)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            let text = String::from_utf16_lossy(&units);
            if let Some(value) = extract_authst(text.as_bytes()) {
                return Some(value);
            }
        }
        None
    }

    fn find_all<'a>(data: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
        data.windows(needle.len())
            .enumerate()
            .filter_map(move |(index, value)| (value == needle).then_some(index))
    }

    fn token_after(data: &[u8]) -> Option<String> {
        let start = data
            .iter()
            .position(|byte| !matches!(*byte, b':' | b'=' | b' ' | b'\t' | b'\\' | b'\"'))?;
        let value = data[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(**byte, b'-' | b'_' | b'='))
            .copied()
            .collect::<Vec<_>>();
        (value.len() > 20).then(|| String::from_utf8_lossy(&value).into_owned())
    }
}
