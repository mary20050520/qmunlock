pub mod credentials;
pub mod decrypt;
pub mod ekey;
pub mod footer;
pub mod qmc2;
pub mod qq_library;
pub mod tags;

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub available: bool,
    pub platform: String,
    pub account_hint: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub supported: bool,
    /// 文件类别：`enc` 为 QQ 音乐加密格式，`plain` 为普通音频
    pub kind: String,
    pub format: Option<String>,
    pub song_mid: Option<String>,
    pub resource_filename: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecryptResult {
    pub input: String,
    pub output: Option<String>,
    pub ok: bool,
    pub format: Option<String>,
    pub error: Option<String>,
    /// 封面处理结果（仅在启用写封面时有值）
    pub cover: Option<String>,
    /// 歌词处理结果（仅在启用抓歌词时有值）
    pub lyrics: Option<String>,
    /// QQ 音乐库链接结果（仅在启用该选项时有值）
    pub library: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub phase: String,
    pub input: String,
    pub current: u64,
    pub total: u64,
    /// 整批任务进度：(已完成文件数 + 当前文件内占比) / 总文件数。用于顶栏与底栏。
    pub percent: u8,
    /// 当前文件自身进度 0-100。用于队列里每一行的进度条。
    /// 两者必须分开：共用一个值会让第一行瞬间走完、最后一行迟迟不满。
    pub file_percent: u8,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DecryptOptions {
    pub output_mode: OutputMode,
    pub manual_ekey: Option<String>,
    /// 输出后写入专辑封面（FLAC；macOS 同时设置访达图标）
    #[serde(default)]
    pub embed_cover: bool,
    /// 输出后抓取 LRC 歌词
    #[serde(default)]
    pub fetch_lyrics: bool,
    /// 歌词输出目录，留空则与音频同目录
    #[serde(default)]
    pub lyrics_dir: Option<String>,
    /// 输出后把文件链接到 QQ 音乐本地库（仅 macOS 有效，需退出 QQ 音乐）
    #[serde(default)]
    pub link_library: bool,
    /// 普通音频是否先复制副本再增强（false 表示原位写入）
    #[serde(default)]
    pub plain_copy: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Original,
    Mp3,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub uin: String,
    pub authst: String,
    pub login_type: String,
}

#[derive(Debug, Clone)]
pub struct MusicExFooter {
    pub audio_length: u64,
    pub song_mid: String,
    pub filename: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.into())
    }
}
impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|x| x.to_str())
        .map(|x| x.to_ascii_lowercase())
}

/// QQ 音乐加密格式（需要解密）。
pub fn enc_path(path: &Path) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("mgg") | Some("mflac") | Some("mmp4")
    )
}

/// 普通音频（无需解密，只做封面 / 歌词 / 曲库增强）。
pub fn plain_audio_path(path: &Path) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("flac") | Some("mp3") | Some("m4a") | Some("ogg") | Some("opus") | Some("wav")
    )
}

/// 本工具能处理的文件：加密格式或普通音频。
pub fn supported_path(path: &Path) -> bool {
    enc_path(path) || plain_audio_path(path)
}
