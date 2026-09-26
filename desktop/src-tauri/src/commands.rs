use crate::core::{
    self, credentials, decrypt, ekey, qq_library, tags, DecryptOptions, DecryptResult, Error,
    FileInfo, ProgressEvent,
};
use std::path::{Path, PathBuf};
use tauri::Emitter;

#[tauri::command]
pub fn check_credentials() -> core::CredentialStatus {
    credentials::status()
}

/// 运行平台标识（`macos` / `windows` / `linux`），供前端门控平台专属功能。
///
/// 前端不能靠 UA 或凭据接口推断平台：凭据读取失败时 platform 会是 unknown，
/// 会把 macOS 独有的曲库功能误隐藏。这里直接取编译期目标系统，永远可靠。
#[tauri::command]
pub fn os_platform() -> String {
    std::env::consts::OS.to_owned()
}

/// QQ 音乐本地库链接状态（待办数量、数据库是否找到、客户端是否在运行）。
#[tauri::command]
pub fn library_status() -> qq_library::LibraryStatus {
    qq_library::status()
}

/// 补做所有待办的 QQ 音乐库链接。
#[tauri::command]
pub fn flush_library_links() -> String {
    qq_library::flush_pending()
}

#[tauri::command]
pub fn get_file_info(path: String) -> FileInfo {
    classify(&PathBuf::from(&path))
}

/// 识别单个文件：区分加密格式与普通音频，普通音频无需解密即可增强。
fn classify(path: &Path) -> FileInfo {
    let display = path.display().to_string();
    if core::enc_path(path) {
        match decrypt::info(path) {
            Ok(footer) => FileInfo {
                path: display,
                supported: true,
                kind: "enc".into(),
                format: Some("musicex/QMC2".into()),
                song_mid: Some(footer.song_mid),
                resource_filename: Some(footer.filename),
                error: None,
            },
            Err(error) => FileInfo {
                path: display,
                supported: false,
                kind: "enc".into(),
                format: None,
                song_mid: None,
                resource_filename: None,
                error: Some(error.to_string()),
            },
        }
    } else if core::plain_audio_path(path) {
        let format = path
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
            .unwrap_or_default();
        FileInfo {
            path: display,
            supported: true,
            kind: "plain".into(),
            format: Some(format),
            song_mid: None,
            resource_filename: None,
            error: None,
        }
    } else {
        FileInfo {
            path: display,
            supported: false,
            kind: "unknown".into(),
            format: None,
            song_mid: None,
            resource_filename: None,
            error: Some("不支持的文件类型".into()),
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub files: Vec<String>,
    pub infos: Vec<FileInfo>,
}

#[tauri::command]
pub fn scan_paths(app: tauri::AppHandle, paths: Vec<String>) -> ScanResult {
    let inputs = expand_paths(paths);
    let total = inputs.len().max(1) as u64;
    let mut infos = Vec::with_capacity(inputs.len());
    emit_progress(&app, "scan", "", 0, total, "正在扫描拖入的文件和文件夹");
    for (index, path) in inputs.iter().enumerate() {
        let info = classify(path);
        let current = (index + 1) as u64;
        emit_progress(
            &app,
            "parse",
            &info.path,
            current,
            total,
            &format!("正在解析 {}/{} 个文件", current, inputs.len()),
        );
        infos.push(info);
    }
    ScanResult {
        files: inputs
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        infos,
    }
}

#[tauri::command]
pub async fn decrypt_paths(
    app: tauri::AppHandle,
    paths: Vec<String>,
    output_dir: Option<String>,
    options: DecryptOptions,
) -> Vec<DecryptResult> {
    let inputs = expand_paths(paths);
    let output = output_dir.as_deref().map(Path::new);
    let credentials = if options
        .manual_ekey
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        credentials::load().ok()
    } else {
        None
    };
    let mut results = Vec::with_capacity(inputs.len());
    let total = inputs.len().max(1) as u64;
    for (index, input) in inputs.iter().enumerate() {
        let result = decrypt_one(
            &app,
            input,
            output,
            &options,
            credentials.as_ref(),
            (index + 1) as u64,
            total,
        )
        .await;
        results.push(result);
    }
    emit_frac(&app, "complete", "", total, total, 1.0, "任务处理完成");
    results
}

async fn decrypt_one(
    app: &tauri::AppHandle,
    input: &Path,
    output_dir: Option<&Path>,
    options: &DecryptOptions,
    credentials: Option<&core::Credentials>,
    file_index: u64,
    file_total: u64,
) -> DecryptResult {
    let input_name = input.display().to_string();
    if core::plain_audio_path(input) {
        return enhance_plain(app, input, output_dir, options, file_index, file_total).await;
    }
    emit_frac(
        app,
        "parse",
        &input_name,
        file_index,
        file_total,
        FRAC_PARSE,
        "正在读取 musicex footer",
    );
    let run = async {
        let footer = decrypt::info(input)?;
        emit_frac(
            app,
            "ekey",
            &input_name,
            file_index,
            file_total,
            FRAC_EKEY,
            "正在获取 ekey",
        );
        let key = match options
            .manual_ekey
            .as_deref()
            .filter(|key| !key.trim().is_empty())
        {
            Some(key) => key.to_owned(),
            None => {
                let credentials =
                    credentials.ok_or("无法自动获取 ekey：请登录 QQ 音乐，或粘贴手动 ekey")?;
                ekey::fetch(&footer, credentials, credentials::api_platform()).await?
            }
        };
        let progress_app = app.clone();
        let progress_input = input_name.clone();
        let fi = file_index;
        let ft = file_total;
        decrypt::decrypt_file_with_progress(
            input,
            output_dir,
            &footer,
            &key,
            &options.output_mode,
            move |current, total, phase| {
                let ratio = if total == 0 {
                    1.0
                } else {
                    current as f64 / total as f64
                };
                let (frac, message) = if phase == "transcode" {
                    (FRAC_TRANSCODE, "正在转换为 MP3")
                } else {
                    (
                        FRAC_DECRYPT_BASE + FRAC_DECRYPT_SPAN * ratio,
                        "正在解密音频",
                    )
                };
                emit_frac(&progress_app, phase, &progress_input, fi, ft, frac, message);
            },
        )
    }
    .await;
    match run {
        Ok((output, format)) => {
            let song_mid = decrypt::info(input)
                .map(|footer| footer.song_mid)
                .unwrap_or_default();
            let (cover, library) = if options.embed_cover || options.link_library {
                let notes = decorate_output(
                    app,
                    &output,
                    &format,
                    &song_mid,
                    &input_name,
                    options,
                    BatchPos {
                        index: file_index,
                        total: file_total,
                    },
                )
                .await;
                (notes.cover, notes.library)
            } else {
                (None, None)
            };
            let lyrics = if options.fetch_lyrics {
                Some(
                    write_lyrics(
                        app,
                        &output,
                        options,
                        &song_mid,
                        &input_name,
                        file_index,
                        file_total,
                    )
                    .await,
                )
            } else {
                None
            };
            DecryptResult {
                input: input_name,
                output: Some(output.display().to_string()),
                ok: true,
                format: Some(format),
                error: None,
                cover,
                lyrics,
                library,
            }
        }
        Err(error) => DecryptResult {
            input: input_name,
            output: None,
            ok: false,
            format: None,
            error: Some(error.to_string()),
            cover: None,
            lyrics: None,
            library: None,
        },
    }
}

/// 「副本」模式的目标路径：既不能等于源路径，也不能覆盖目录里已有的文件。
///
/// 未选输出目录时目录就是源目录，同名会与源同路径，「副本」就变成改原文件；
/// 选了目录也可能撞同名文件。两种情况都追加「副本 / 副本 2 …」序号避开。
fn unique_copy_target(input: &Path, dir: &Path) -> PathBuf {
    let name = input
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    let stem = input
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = input
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let mut dest = dir.join(name);
    let mut index = 2u32;
    while dest.as_path() == input || dest.exists() {
        let suffix = if index == 2 {
            String::new()
        } else {
            format!(" {}", index - 1)
        };
        dest = dir.join(format!("{stem} 副本{suffix}{ext}"));
        index += 1;
    }
    dest
}

/// 普通音频通道：不解密，只做封面 / 歌词 / 曲库增强。
///
/// 曲库信息按文件名（去掉扩展名）检索；`plain_copy` 为真时先复制副本再改副本，
/// 否则原位写入。macOS 独有的访达图标与 QQ 音乐属性仍由 cfg 门控，其他平台自动跳过。
async fn enhance_plain(
    app: &tauri::AppHandle,
    input: &Path,
    output_dir: Option<&Path>,
    options: &DecryptOptions,
    file_index: u64,
    file_total: u64,
) -> DecryptResult {
    let input_name = input.display().to_string();
    let format = input
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.to_ascii_lowercase())
        .unwrap_or_default();
    emit_frac(
        app,
        "parse",
        &input_name,
        file_index,
        file_total,
        FRAC_PARSE,
        "正在识别普通音频",
    );
    let run = async {
        let target = if options.plain_copy {
            let dir = output_dir
                .map(Path::to_path_buf)
                .or_else(|| input.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."));
            std::fs::create_dir_all(&dir)?;
            let dest = unique_copy_target(input, &dir);
            std::fs::copy(input, &dest)?;
            dest
        } else {
            input.to_path_buf()
        };

        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        // 文件自带标签比文件名可靠：优先用它构造检索词，并校验检索结果是否同一首歌
        let embedded = tags::read_embedded_tags(input);
        let query = match embedded.as_ref() {
            Some(found) if !found.title.is_empty() => {
                if found.artist.is_empty() {
                    found.title.clone()
                } else {
                    format!("{} {}", found.title, found.artist)
                }
            }
            _ => stem,
        };
        emit_frac(
            app,
            "cover",
            &input_name,
            file_index,
            file_total,
            FRAC_COVER,
            "正在检索曲库",
        );
        // 检索返回全部候选，再结合文件标签逐条校验挑出同一首歌，避免错配版本
        let candidates = tags::search_songs(&query).await?;
        let meta = tags::pick_matching_meta(&query, candidates, embedded.as_ref())?;

        let mut notes: Vec<String> = Vec::new();
        // 「封面」= 内嵌封面 + 访达自定义图标（macOS），理由同 decorate_output
        let image = if options.embed_cover {
            match tags::fetch_cover(&meta.album_mid).await {
                Ok(image) => Some(image),
                Err(error) => {
                    notes.push(format!("未获取封面：{error}"));
                    None
                }
            }
        } else {
            None
        };

        if options.embed_cover {
            if let Some(image) = image.as_ref() {
                if format == "flac" {
                    let description = if meta.singers.is_empty() {
                        meta.title.clone()
                    } else {
                        format!("{} - {}", meta.title, meta.singers)
                    };
                    match tags::embed_cover_into_flac(&target, image, &description) {
                        Ok(()) => notes.push(format!("封面《{}》", meta.album)),
                        Err(error) => notes.push(format!("未写入封面：{error}")),
                    }
                } else {
                    notes.push(format!(
                        "内嵌封面仅支持 FLAC，当前为 {}",
                        format.to_uppercase()
                    ));
                }
                if cfg!(target_os = "macos") {
                    match tags::set_finder_icon(&target, image) {
                        Ok(()) => notes.push("访达图标".to_owned()),
                        Err(error) => notes.push(format!("访达图标未设置：{error}")),
                    }
                }
            }
        }

        let lyrics = if options.fetch_lyrics {
            Some(
                write_lyrics(
                    app,
                    &target,
                    options,
                    &meta.song_mid,
                    &input_name,
                    file_index,
                    file_total,
                )
                .await,
            )
        } else {
            None
        };

        // 「曲库」= QQ 音乐扩展属性 + 本地库链接（访达图标已归「封面」）
        let library = if options.link_library {
            emit_frac(
                app,
                "library",
                &input_name,
                file_index,
                file_total,
                FRAC_LIBRARY,
                "正在链接 QQ 音乐本地库",
            );
            if cfg!(target_os = "macos") {
                match tags::write_qq_attributes(&target, &meta) {
                    Ok(note) if !note.is_empty() => notes.push("QQ 属性".to_owned()),
                    Ok(_) => {}
                    Err(error) => notes.push(format!("QQ 属性未写入：{error}")),
                }
            }
            let entry = qq_library::LibraryEntry {
                file: target.display().to_string(),
                song_id: meta.song_id,
                title: meta.title.clone(),
                singer: meta.singers.clone(),
                album: meta.album.clone(),
                album_mid: meta.album_mid.clone(),
            };
            let note = qq_library::link_or_queue(&entry);
            (!note.is_empty()).then_some(short_library_note(&note))
        } else {
            None
        };

        Ok::<_, Error>((target, notes, lyrics, library))
    }
    .await;

    match run {
        Ok((target, notes, lyrics, library)) => DecryptResult {
            input: input_name,
            output: Some(target.display().to_string()),
            ok: true,
            format: Some(format),
            error: None,
            cover: (!notes.is_empty()).then(|| notes.join(" · ")),
            lyrics,
            library,
        },
        Err(error) => DecryptResult {
            input: input_name,
            output: None,
            ok: false,
            format: None,
            error: Some(error.to_string()),
            cover: None,
            lyrics: None,
            library: None,
        },
    }
}

/// 输出后的增强处理结果。
struct EnhanceNotes {
    cover: Option<String>,
    library: Option<String>,
}

/// 把 qq_library 返回的长说明压缩成界面上能一眼扫完的短标签。
fn short_library_note(note: &str) -> String {
    if note.contains("稍后补做") {
        "曲库待补做".to_owned()
    } else if note.contains("未完成") || note.contains("失败") {
        "曲库失败".to_owned()
    } else {
        "曲库".to_owned()
    }
}

/// 批次位置：当前是第几个文件、共几个文件。
/// 把两个计数器合成一个参数，避免 decorate_output 触发
/// clippy::too_many_arguments（上限 7 个）。
#[derive(Debug, Clone, Copy)]
struct BatchPos {
    index: u64,
    total: u64,
}

/// 输出后的增强处理。
///
/// 职责划分：
/// - 「封面」= 内嵌封面（仅 FLAC）+ 访达自定义图标（仅 macOS）。内嵌封面只存在于
///   文件内部、访达里看不见，用户真正「看到封面」的是访达图标，所以二者必须同属
///   一个开关，否则勾了封面会像没生效；
/// - 「曲库」= QQ 音乐扩展属性 + 本地库链接（均仅 macOS）。
///
/// 每一步都是尽力而为：失败只记录到任务报告，不影响已经完成的解密结果。
async fn decorate_output(
    app: &tauri::AppHandle,
    output: &Path,
    format: &str,
    song_mid: &str,
    input_name: &str,
    options: &DecryptOptions,
    pos: BatchPos,
) -> EnhanceNotes {
    let mut notes: Vec<String> = Vec::new();
    emit_frac(
        app,
        "cover",
        input_name,
        pos.index,
        pos.total,
        FRAC_COVER,
        "正在获取曲库信息与封面",
    );
    let meta = match tags::resolve_song(song_mid).await {
        Ok(meta) => meta,
        Err(error) => {
            return EnhanceNotes {
                cover: options.embed_cover.then(|| format!("未写入封面：{error}")),
                library: options.link_library.then(|| format!("曲库未完成：{error}")),
            }
        }
    };

    // 「封面」= 内嵌封面 + 访达自定义图标（macOS）。
    // 内嵌封面只在文件内部，访达里看不见；用户真正「看到封面」的是访达图标，
    // 所以两者必须同属一个开关，否则勾了封面会像没生效。
    let image = if options.embed_cover {
        match tags::fetch_cover(&meta.album_mid).await {
            Ok(image) => Some(image),
            Err(error) => {
                notes.push(format!("未获取封面：{error}"));
                None
            }
        }
    } else {
        None
    };

    if options.embed_cover {
        if let Some(image) = image.as_ref() {
            if format == "flac" {
                let description = if meta.singers.is_empty() {
                    meta.title.clone()
                } else {
                    format!("{} - {}", meta.title, meta.singers)
                };
                match tags::embed_cover_into_flac(output, image, &description) {
                    Ok(()) => notes.push(format!("封面《{}》", meta.album)),
                    Err(error) => notes.push(format!("未写入封面：{error}")),
                }
            } else {
                notes.push(format!(
                    "内嵌封面仅支持 FLAC，当前为 {}",
                    format.to_uppercase()
                ));
            }
            if cfg!(target_os = "macos") {
                match tags::set_finder_icon(output, image) {
                    Ok(()) => notes.push("访达图标".to_owned()),
                    Err(error) => notes.push(format!("访达图标未设置：{error}")),
                }
            }
        }
    }

    let library = if options.link_library {
        emit_frac(
            app,
            "library",
            input_name,
            pos.index,
            pos.total,
            FRAC_LIBRARY,
            "正在链接 QQ 音乐本地库",
        );
        if cfg!(target_os = "macos") {
            // QQ 音乐扩展属性是 macOS 独有的文件机制，其他平台不执行
            match tags::write_qq_attributes(output, &meta) {
                Ok(note) if !note.is_empty() => notes.push("QQ 属性".to_owned()),
                Ok(_) => {}
                Err(error) => notes.push(format!("QQ 属性未写入：{error}")),
            }
        }
        let entry = qq_library::LibraryEntry {
            file: output.display().to_string(),
            song_id: meta.song_id,
            title: meta.title.clone(),
            singer: meta.singers.clone(),
            album: meta.album.clone(),
            album_mid: meta.album_mid.clone(),
        };
        let note = qq_library::link_or_queue(&entry);
        (!note.is_empty()).then_some(short_library_note(&note))
    } else {
        None
    };

    EnhanceNotes {
        cover: (!notes.is_empty()).then(|| notes.join(" · ")),
        library,
    }
}

/// 抓取 LRC 歌词并写入指定目录（留空则与音频同目录）。
async fn write_lyrics(
    app: &tauri::AppHandle,
    output: &Path,
    options: &DecryptOptions,
    song_mid: &str,
    input_name: &str,
    file_index: u64,
    file_total: u64,
) -> String {
    let directory = match options
        .lyrics_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => PathBuf::from(value),
        None => output.parent().unwrap_or(Path::new(".")).to_path_buf(),
    };
    emit_frac(
        app,
        "lyrics",
        input_name,
        file_index,
        file_total,
        FRAC_LYRICS,
        "正在获取歌词",
    );
    let lyric = match tags::fetch_lyric(song_mid).await {
        Ok(lyric) => lyric,
        Err(error) => return format!("未获取歌词：{error}"),
    };
    match tags::write_lyric(output, &directory, &lyric) {
        Ok(path) => {
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("歌词 {name}")
        }
        Err(error) => format!("未写入歌词：{error}"),
    }
}

fn emit_progress(
    app: &tauri::AppHandle,
    phase: &str,
    input: &str,
    current: u64,
    total: u64,
    message: &str,
) {
    let percent = if total == 0 {
        0
    } else {
        ((current.min(total) as f64 / total as f64) * 100.0).round() as u8
    };
    let _ = app.emit(
        "decrypt-progress",
        ProgressEvent {
            phase: phase.into(),
            input: input.into(),
            current,
            total,
            percent,
            // 这个入口只用于扫描等无具体文件的阶段，行内进度条不会激活
            file_percent: percent,
            message: message.into(),
        },
    );
}

/// 以「整批任务」为口径发射进度：`percent = (已完成文件数 + 当前文件内占比) / 总文件数`。
///
/// 旧实现让每个阶段各自从 0/1 开始发射（ekey 0%、封面 0%……），而解密本身是瞬时的、
/// 耗时全在网络等待上，于是界面长时间停在 0% 再一次性跳 100%。这里把阶段占比折算进
/// 当前文件的那一份，进度才会平滑地从 0 走到 100。
fn emit_frac(
    app: &tauri::AppHandle,
    phase: &str,
    input: &str,
    file_index: u64,
    file_total: u64,
    frac: f64,
    message: &str,
) {
    let done = file_index.saturating_sub(1) as f64;
    let clamped = frac.clamp(0.0, 1.0);
    let percent = if file_total == 0 {
        100
    } else {
        (((done + clamped) / file_total as f64) * 100.0).round() as u8
    };
    let _ = app.emit(
        "decrypt-progress",
        ProgressEvent {
            phase: phase.into(),
            input: input.into(),
            current: file_index.min(file_total.max(1)),
            total: file_total,
            percent,
            // 行内进度条只看当前文件自己走到哪，不受批次影响
            file_percent: (clamped * 100.0).round() as u8,
            message: message.into(),
        },
    );
}

/// 单个文件内部的阶段占比，供 emit_frac 折算成整批进度。
const FRAC_PARSE: f64 = 0.05;
const FRAC_EKEY: f64 = 0.15;
const FRAC_DECRYPT_BASE: f64 = 0.20;
const FRAC_DECRYPT_SPAN: f64 = 0.60;
const FRAC_TRANSCODE: f64 = 0.82;
const FRAC_COVER: f64 = 0.88;
const FRAC_LYRICS: f64 = 0.93;
const FRAC_LIBRARY: f64 = 0.97;

fn expand_paths(paths: Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths.into_iter().map(PathBuf::from) {
        if path.is_file() {
            if core::supported_path(&path) {
                files.push(path);
            }
        } else if path.is_dir() {
            collect(&path, &mut files);
        }
    }
    files.sort();
    files.dedup();
    files
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if core::supported_path(&path) {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_target_avoids_source_and_existing_files() {
        let dir = std::env::temp_dir().join(format!("qmunlock-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("song.flac");
        std::fs::write(&source, b"x").unwrap();

        // 未选输出目录时目录即源目录，必须避开源路径，否则「副本」变成改原文件
        let first = unique_copy_target(&source, &dir);
        assert_ne!(first, source);
        assert_eq!(first.file_name().unwrap(), "song 副本.flac");

        std::fs::write(&first, b"x").unwrap();
        let second = unique_copy_target(&source, &dir);
        assert_eq!(second.file_name().unwrap(), "song 副本 2.flac");

        // 选了空目录：直接用原名，不加后缀，也不覆盖任何东西
        let other = dir.join("out");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(unique_copy_target(&source, &other), other.join("song.flac"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
