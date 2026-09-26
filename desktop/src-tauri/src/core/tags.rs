//! 输出后的增强处理：抓歌词、写封面、在 macOS 上设置访达图标。
//!
//! 说明：
//! - 歌词与封面都按 footer 里的歌曲 MID 向 QQ 音乐公开接口取回，只读、不涉及账号凭据。
//! - 封面写进 FLAC 的 PICTURE 元数据块，或 MP3 的 ID3 APIC 帧。
//! - 访达图标是 macOS 独有机制（资源分支 + FinderInfo 标志），仅在 macOS 上执行；
//!   其他平台该步骤为空操作，保持跨平台一致。

use super::{Error, Result};
use id3::{
    frame::{
        Lyrics, Picture, PictureType, SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat,
    },
    Tag, TagLike, Version,
};
use serde_json::Value;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SONG_INFO_URL: &str = "https://c.y.qq.com/v8/fcg-bin/fcg_play_single_song.fcg";
const SEARCH_URL: &str = "https://c.y.qq.com/soso/fcgi-bin/search_for_qq_cp";
const LYRIC_URL: &str = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";
const COVER_URL: &str = "https://y.gtimg.cn/music/photo_new/T002R{s}x{s}M000{mid}.jpg";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

const BLOCK_PICTURE: u8 = 6;
const BLOCK_VORBIS_COMMENT: u8 = 4;
/// FLAC PICTURE 块的图片类型：3 = 正面封面。替换封面时只动这一类，
/// 封底（4）、艺术家（8）等其他内嵌图片必须原样保留。
const PICTURE_TYPE_FRONT: u32 = 3;

#[derive(Debug, Clone)]
pub struct SongMeta {
    pub album_mid: String,
    pub album: String,
    /// 专辑艺人；只有曲库明确返回时才写入，不用歌曲艺人猜测。
    pub album_singers: String,
    pub title: String,
    pub singers: String,
    /// 曲库公开的首发日期，可能是年或完整日期。
    pub release_date: String,
    /// 专辑内曲序；曲库没有提供时为 None。
    pub track_number: Option<u32>,
    /// 碟序；QQ 音乐的 `index_cd` 从 0 开始，写入标签时转为从 1 开始。
    pub disc_number: Option<u32>,
    /// 歌曲 MID，抓歌词需要
    pub song_mid: String,
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

fn collect_names(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("、")
        })
        .unwrap_or_default()
}

async fn get_json(url: &str) -> Result<Value> {
    let response = http()
        .get(url)
        .header("Referer", "https://y.qq.com/")
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

/// 用歌曲 MID 取专辑信息（封面地址需要专辑 MID）。
pub async fn resolve_song(song_mid: &str) -> Result<SongMeta> {
    if song_mid.is_empty() {
        return Err(Error::from("缺少歌曲 MID，无法查询曲库信息"));
    }
    let url = format!(
        "{SONG_INFO_URL}?songmid={song_mid}&platform=yqq&format=json&needNewCode=0&new_json=1"
    );
    let value = get_json(&url).await?;
    let song = value
        .pointer("/data/0")
        .ok_or_else(|| Error::from("曲库未返回该歌曲信息"))?;
    let album_mid = song
        .pointer("/album/mid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let album = song
        .pointer("/album/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // title 带完整后缀（如 "消愁 (Live)"），name 是干净标题，优先用 title
    let title = song
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| song.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let singers = collect_names(song.get("singer"));
    let album_singers = collect_names(song.pointer("/album/singer"));
    let release_date = song
        .get("time_public")
        .or_else(|| song.pointer("/album/time_public"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let track_number = song
        .get("index_album")
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0);
    let disc_number = song
        .get("index_cd")
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .map(|number| number.saturating_add(1));
    if album_mid.is_empty() {
        return Err(Error::from("曲库未返回专辑 MID"));
    }
    Ok(SongMeta {
        album_mid,
        album,
        album_singers,
        title,
        singers,
        release_date,
        track_number,
        disc_number,
        song_mid: song_mid.to_owned(),
    })
}

/// 从搜索结果的单条记录里提取曲库信息；字段缺失时返回 None。
fn meta_from_search_item(item: &Value) -> Option<SongMeta> {
    let song_mid = item
        .get("songmid")
        .or_else(|| item.get("mid"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // songmid 为 "0" 或空的是聚合条目，本身没有封面与歌词，调用方需下钻 grp
    if song_mid.is_empty() || song_mid == "0" {
        return None;
    }
    let album_mid = item
        .get("albummid")
        .or_else(|| item.get("albumMid"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if album_mid.is_empty() {
        return None;
    }
    let album = item
        .get("albumname")
        .or_else(|| item.get("albumName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let title = item
        .get("songname")
        .or_else(|| item.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let singers = collect_names(item.get("singer"));
    Some(SongMeta {
        album_mid,
        album,
        album_singers: String::new(),
        title,
        singers,
        release_date: String::new(),
        track_number: None,
        disc_number: None,
        song_mid,
    })
}

/// 用「歌名 / 歌手」关键词反查曲库条目，返回全部有效候选。
///
/// 供普通音频（没有 musicex footer、拿不到歌曲 MID）使用：拿文件名当关键词检索。
/// 搜索结果里 `songmid` 为 "0" 的是聚合条目，真实 MID 藏在它的 `grp` 子项里，
/// 这里会自动下钻一层。候选按曲库返回顺序保留，由调用方结合文件自带标签挑选。
pub async fn search_songs(query: &str) -> Result<Vec<SongMeta>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Error::from("缺少检索关键词，无法查询曲库信息"));
    }
    let url = format!(
        "{SEARCH_URL}?g_tk=5381&inCharset=utf8&outCharset=utf-8&notice=0&platform=yqq.json\
&needNewCode=0&format=json&w={}",
        percent_encode(query)
    );
    let value = get_json(&url).await?;
    let list = value
        .pointer("/data/song/list")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::from("曲库搜索未返回结果"))?;
    let mut candidates = Vec::new();
    for item in list {
        if let Some(meta) = meta_from_search_item(item) {
            candidates.push(meta);
        }
        if let Some(group) = item.get("grp").and_then(Value::as_array) {
            for child in group {
                if let Some(meta) = meta_from_search_item(child) {
                    candidates.push(meta);
                }
            }
        }
    }
    Ok(candidates)
}

/// 从检索候选里挑出与文件指向同一首歌的条目。
///
/// 文件有标签时按曲库顺序逐条校验、取第一个匹配项：只取第一条会在它与标签不符时
/// 漏掉后面正确的结果；文件无标签时取第一个候选，维持按文件名检索的原行为。
pub fn pick_matching_meta(
    query: &str,
    candidates: Vec<SongMeta>,
    embedded: Option<&EmbeddedTags>,
) -> Result<SongMeta> {
    match embedded {
        Some(tags) => {
            let total = candidates.len();
            let picked = candidates
                .into_iter()
                .find(|meta| matches_embedded_tags(meta, tags));
            picked.ok_or_else(|| {
                Error::from(format!(
                    "曲库 {total} 条候选均与文件标签《{}》{} 不符，已跳过",
                    tags.title, tags.artist
                ))
            })
        }
        None => candidates
            .into_iter()
            .next()
            .ok_or_else(|| Error::from(format!("曲库中检索不到「{query}」"))),
    }
}

/// 文件自带标签（标题 / 艺术家 / 专辑）。
///
/// 普通音频没有 musicex footer，只能靠检索猜身份；文件内已有的标签是比文件名
/// 更可靠的依据，用来构造检索词并校验检索结果是否指向同一首歌。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbeddedTags {
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// 读取文件自带标签：FLAC 走 Vorbis 注释，MP3 走 ID3v2；其他格式或无标签返回 None。
pub fn read_embedded_tags(path: &Path) -> Option<EmbeddedTags> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("flac") => read_flac_tags(path),
        Some("mp3") => read_id3v2_tags(path),
        _ => None,
    }
}

fn read_flac_tags(path: &Path) -> Option<EmbeddedTags> {
    let mut file = std::fs::File::open(path).ok()?;
    let blocks = read_blocks(&mut file).ok()?;
    let comment = blocks
        .iter()
        .find(|(kind, _)| *kind == BLOCK_VORBIS_COMMENT)
        .map(|(_, data)| data)?;
    vorbis_tags(comment)
}

/// Vorbis 注释块：供应商字符串长度 + 内容 + 条目数 + 每条「NAME=value」。
fn vorbis_tags(data: &[u8]) -> Option<EmbeddedTags> {
    let mut tags = EmbeddedTags::default();
    let mut offset = 0usize;
    let vendor = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize;
    offset += 4 + vendor;
    let count = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize;
    offset += 4;
    for _ in 0..count {
        let length = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize;
        offset += 4;
        let entry = String::from_utf8_lossy(data.get(offset..offset + length)?).to_string();
        offset += length;
        let Some((name, value)) = entry.split_once('=') else {
            continue;
        };
        match name.to_ascii_uppercase().as_str() {
            "TITLE" => tags.title = value.to_owned(),
            "ARTIST" | "ALBUMARTIST" if tags.artist.is_empty() => {
                tags.artist = value.to_owned();
            }
            "ALBUM" => tags.album = value.to_owned(),
            _ => {}
        }
    }
    if tags.title.is_empty() && tags.artist.is_empty() && tags.album.is_empty() {
        None
    } else {
        Some(tags)
    }
}

/// ID3v2.3 / 2.4：只取 TIT2（标题）、TPE1（艺术家）、TALB（专辑）三个文本帧。
///
/// 只按 ID3 头部声明的长度读取标签区，不把整个音频载入内存；APIC、COMM 等无关帧
/// 按帧长跳过并继续读取后续帧，歌名 / 歌手排在它们后面时也不会漏。
fn read_id3v2_tags(path: &Path) -> Option<EmbeddedTags> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header).ok()?;
    if &header[0..3] != b"ID3" {
        return None;
    }
    let version = header[3];
    let size = synchsafe(&header[6..10])? as usize;
    let mut data = Vec::new();
    file.take(size as u64).read_to_end(&mut data).ok()?;
    let end = data.len();
    let mut tags = EmbeddedTags::default();
    let mut offset = 0usize;
    while offset + 10 <= end {
        let Ok(id) = std::str::from_utf8(data.get(offset..offset + 4)?) else {
            break;
        };
        let frame_size = if version >= 4 {
            synchsafe(data.get(offset + 4..offset + 8)?)? as usize
        } else {
            u32::from_be_bytes(data.get(offset + 4..offset + 8)?.try_into().ok()?) as usize
        };
        if frame_size == 0 {
            break;
        }
        let Some(body) = data.get(offset + 10..offset + 10 + frame_size) else {
            break;
        };
        match id {
            "TIT2" => tags.title = id3_text(body),
            "TPE1" if tags.artist.is_empty() => tags.artist = id3_text(body),
            "TALB" => tags.album = id3_text(body),
            // 无关帧（封面、评论、私有帧等）：按帧长跳过，继续读后续帧
            _ => {}
        }
        offset += 10 + frame_size;
    }
    if tags.title.is_empty() && tags.artist.is_empty() && tags.album.is_empty() {
        None
    } else {
        Some(tags)
    }
}

/// ID3 的 synchsafe 整数：每字节只用低 7 位。
fn synchsafe(bytes: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    Some(
        u32::from(bytes[0] & 0x7F) << 21
            | u32::from(bytes[1] & 0x7F) << 14
            | u32::from(bytes[2] & 0x7F) << 7
            | u32::from(bytes[3] & 0x7F),
    )
}

/// ID3 文本帧首字节是编码：0 Latin-1、1 UTF-16 带 BOM、2 UTF-16BE、3 UTF-8。
fn id3_text(body: &[u8]) -> String {
    let Some((encoding, text)) = body.split_first() else {
        return String::new();
    };
    let decoded = match encoding {
        1 => {
            let big_endian = text.get(0..2) == Some(&[0xFE, 0xFF][..]);
            let skip = if text.len() >= 2
                && ((text[0] == 0xFF && text[1] == 0xFE) || (text[0] == 0xFE && text[1] == 0xFF))
            {
                2
            } else {
                0
            };
            let units: Vec<u16> = text[skip..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    if big_endian {
                        u16::from_be_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_le_bytes([pair[0], pair[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        }
        2 => {
            let units: Vec<u16> = text
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        // Latin-1 的中文标签几乎不存在，按 UTF-8 近似解析即可
        _ => String::from_utf8_lossy(text).to_string(),
    };
    decoded.trim_matches('\0').trim().to_owned()
}

/// 归一化：小写并去掉空格与括号、间隔号等标点，只留可比较的字符。
fn normalize_for_match(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    '(' | ')' | '（' | '）' | '·' | '-' | '_' | '\'' | '"' | '、' | ',' | '，'
                )
        })
        .collect::<String>()
        .to_lowercase()
}

/// 标题是否同一首：归一化后相等，或一方包含另一方（容忍「(Live)」一类后缀差异）。
fn same_song_name(left: &str, right: &str) -> bool {
    let left = normalize_for_match(left);
    let right = normalize_for_match(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left == right || left.contains(&right) || right.contains(&left)
}

/// 曲库检索结果是否与文件自带标签指向同一首歌。
///
/// 文件没有标签（或只有专辑名）时返回 true：没有可比信息，维持按文件名检索的原行为。
/// 标题、歌手或专辑明显对不上时返回 false，调用方应跳过而不是写错封面 / 歌词 / 曲库；
/// 专辑参与比较是为了区分同名同歌手的不同专辑版本。
pub fn matches_embedded_tags(meta: &SongMeta, tags: &EmbeddedTags) -> bool {
    if tags.title.is_empty() && tags.artist.is_empty() {
        return true;
    }
    if !tags.title.is_empty() && !same_song_name(&meta.title, &tags.title) {
        return false;
    }
    if !tags.album.is_empty() && !meta.album.is_empty() {
        let left = normalize_for_match(&tags.album);
        let right = normalize_for_match(&meta.album);
        if left != right {
            return false;
        }
    }
    if tags.artist.is_empty() || meta.singers.is_empty() {
        return true;
    }
    let wanted = normalize_for_match(
        tags.artist
            .split(['、', ',', '/'])
            .next()
            .unwrap_or_default(),
    );
    if wanted.is_empty() {
        return true;
    }
    meta.singers.split('、').any(|singer| {
        let singer = normalize_for_match(singer);
        !singer.is_empty() && (singer.contains(&wanted) || wanted.contains(&singer))
    })
}

/// 极简百分号编码：只保留检索关键词里安全的字符。
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 按歌曲 MID 取 LRC 歌词文本。
pub async fn fetch_lyric(song_mid: &str) -> Result<String> {
    let url = format!(
        "{LYRIC_URL}?songmid={song_mid}&format=json&nobase64=1&g_tk=5381&loginUin=0&hostUin=0\
&inCharset=utf8&outCharset=utf-8&notice=0&platform=yqq.json&needNewCode=0"
    );
    let value = get_json(&url).await?;
    let lyric = value
        .get("lyric")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if lyric.is_empty() {
        return Err(Error::from("曲库中没有该曲目的歌词"));
    }
    Ok(lyric)
}

/// 下载专辑封面，按 800 / 500 / 300 依次降级。
pub async fn fetch_cover(album_mid: &str) -> Result<Vec<u8>> {
    if album_mid.is_empty() {
        return Err(Error::from("缺少专辑 MID，无法获取封面"));
    }
    for size in [800u32, 500, 300] {
        let url = COVER_URL
            .replace("{s}", &size.to_string())
            .replace("{mid}", album_mid);
        let response = match http()
            .get(&url)
            .header("Referer", "https://y.qq.com/")
            .header("User-Agent", USER_AGENT)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => continue,
        };
        if !response.status().is_success() {
            continue;
        }
        let bytes = response.bytes().await?;
        if bytes.len() > 1024 {
            return Ok(bytes.to_vec());
        }
    }
    Err(Error::from("未能下载专辑封面"))
}

fn jpeg_dimensions(data: &[u8]) -> (u32, u32) {
    let mut index = 2usize;
    while index + 9 < data.len() {
        if data[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = data[index + 1];
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        let is_sof = matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        );
        if is_sof && index + 9 <= data.len() {
            let height = u16::from_be_bytes([data[index + 5], data[index + 6]]) as u32;
            let width = u16::from_be_bytes([data[index + 7], data[index + 8]]) as u32;
            return (width, height);
        }
        index += 2 + length;
    }
    (0, 0)
}

fn png_dimensions(data: &[u8]) -> (u32, u32) {
    if data.len() >= 24 && &data[..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return (width, height);
    }
    (0, 0)
}

fn build_picture_block(image: &[u8], description: &str) -> Vec<u8> {
    let mime = if image.starts_with(b"\x89PNG") {
        "image/png"
    } else {
        "image/jpeg"
    };
    let (width, height) = if mime == "image/png" {
        png_dimensions(image)
    } else {
        jpeg_dimensions(image)
    };
    let desc = description.as_bytes();
    let mut block = Vec::with_capacity(image.len() + desc.len() + 64);
    block.extend_from_slice(&3u32.to_be_bytes()); // 3 = 封面正面
    block.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    block.extend_from_slice(mime.as_bytes());
    block.extend_from_slice(&(desc.len() as u32).to_be_bytes());
    block.extend_from_slice(desc);
    block.extend_from_slice(&width.to_be_bytes());
    block.extend_from_slice(&height.to_be_bytes());
    block.extend_from_slice(&24u32.to_be_bytes()); // 色深
    block.extend_from_slice(&0u32.to_be_bytes()); // 调色板颜色数
    block.extend_from_slice(&(image.len() as u32).to_be_bytes());
    block.extend_from_slice(image);
    block
}

fn read_blocks(file: &mut std::fs::File) -> Result<Vec<(u8, Vec<u8>)>> {
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"fLaC" {
        return Err(Error::from("不是 FLAC 文件"));
    }
    let mut blocks = Vec::new();
    loop {
        let mut header = [0u8; 4];
        file.read_exact(&mut header)?;
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7F;
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let mut data = vec![0u8; length];
        file.read_exact(&mut data)?;
        blocks.push((kind, data));
        if last {
            break;
        }
    }
    Ok(blocks)
}

fn write_blocks(file: &mut std::fs::File, blocks: &[(u8, Vec<u8>)]) -> Result<()> {
    file.write_all(b"fLaC")?;
    let total = blocks.len();
    for (index, (kind, data)) in blocks.iter().enumerate() {
        let flag = if index + 1 == total { 0x80 } else { 0x00 };
        let length = data.len() as u32;
        file.write_all(&[flag | kind])?;
        file.write_all(&length.to_be_bytes()[1..])?;
        file.write_all(data)?;
    }
    Ok(())
}

/// PICTURE 块数据的前 4 字节是大端图片类型。
fn picture_type(data: &[u8]) -> u32 {
    data.get(0..4)
        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .unwrap_or(u32::MAX)
}

/// 是否为正面封面图（替换时唯一会被移除的一类）。
fn is_front_picture(block: &(u8, Vec<u8>)) -> bool {
    block.0 == BLOCK_PICTURE && picture_type(&block.1) == PICTURE_TYPE_FRONT
}

/// 把封面写入 FLAC 的 PICTURE 块。
///
/// 只替换「正面封面」这一类图片：封底、艺术家照片等其他 PICTURE 块原样保留，
/// 音频数据原样复制。
pub fn embed_cover_into_flac(path: &Path, image: &[u8], description: &str) -> Result<()> {
    let mut source = std::fs::File::open(path)?;
    let blocks = read_blocks(&mut source)?;
    let audio_offset = source.stream_position()?;

    let kept_pictures = blocks
        .iter()
        .filter(|block| block.0 == BLOCK_PICTURE && !is_front_picture(block))
        .count();
    let mut merged: Vec<(u8, Vec<u8>)> = blocks
        .into_iter()
        .filter(|block| !is_front_picture(block))
        .collect();
    let insert_at = merged
        .iter()
        .position(|(kind, _)| *kind == BLOCK_VORBIS_COMMENT)
        .map(|index| index + 1)
        .unwrap_or_else(|| usize::min(1, merged.len()));
    merged.insert(
        insert_at,
        (BLOCK_PICTURE, build_picture_block(image, description)),
    );

    let temporary = temporary_path(path);
    {
        let mut target = std::fs::File::create(&temporary)?;
        write_blocks(&mut target, &merged)?;
        let mut reader = BufReader::new(std::fs::File::open(path)?);
        reader.seek(SeekFrom::Start(audio_offset))?;
        std::io::copy(&mut reader, &mut target)?;
        target.flush()?;
        target.sync_all().ok();
    }

    // 校验：块结构可解析、正面封面只有一张、其他图片数量不变、音频区字节数一致
    let mut check = std::fs::File::open(&temporary)?;
    let check_blocks = read_blocks(&mut check)?;
    let check_audio_offset = check.stream_position()?;
    drop(check);
    let pictures = check_blocks
        .iter()
        .filter(|block| is_front_picture(block))
        .count();
    let kept = check_blocks
        .iter()
        .filter(|block| block.0 == BLOCK_PICTURE && !is_front_picture(block))
        .count();
    let source_audio = std::fs::metadata(path)?.len().saturating_sub(audio_offset);
    let target_audio = std::fs::metadata(&temporary)?
        .len()
        .saturating_sub(check_audio_offset);
    if pictures != 1 || kept != kept_pictures || source_audio != target_audio {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::from(format!(
            "封面写入校验失败（正面封面 {pictures} 个、保留图片 {kept}/{kept_pictures} 张，音频区 {target_audio} / {source_audio} 字节）"
        )));
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// 把封面写入 MP3 的 ID3 APIC（Attached Picture）帧。
///
/// Apple Music 和 Quick Look 都从音频文件的 ID3 标签读取封面，不能只依赖
/// macOS 访达的自定义图标。保留现有标签，只替换正面封面，并把 QQ 音乐明确返回的
/// 曲目信息写入兼容性较好的 ID3v2.3 标签。
pub fn embed_cover_into_mp3(
    path: &Path,
    image: &[u8],
    description: &str,
    meta: &SongMeta,
) -> Result<()> {
    let mut tag = match Tag::read_from_path(path) {
        Ok(tag) => tag,
        // 没有 ID3 标签的 MP3 也应能补写封面；其他损坏/不可读错误则明确返回。
        Err(id3::Error {
            kind: id3::ErrorKind::NoTag,
            ..
        }) => Tag::new(),
        Err(error) => return Err(Error::from(format!("读取 MP3 标签失败：{error}"))),
    };
    tag.remove_picture_by_type(PictureType::CoverFront);
    tag.add_frame(Picture {
        mime_type: "image/jpeg".to_owned(),
        picture_type: PictureType::CoverFront,
        description: description.to_owned(),
        data: image.to_vec(),
    });
    tag.set_title(meta.title.clone());
    if !meta.singers.is_empty() {
        tag.set_artist(meta.singers.clone());
    }
    if !meta.album.is_empty() {
        tag.set_album(meta.album.clone());
    }
    if !meta.album_singers.is_empty() {
        tag.set_album_artist(meta.album_singers.clone());
    }
    if let Some(track) = meta.track_number {
        tag.set_track(track);
    }
    if let Some(disc) = meta.disc_number {
        tag.set_disc(disc);
    }
    if let Some(year) = meta
        .release_date
        .get(0..4)
        .and_then(|value| value.parse::<i32>().ok())
    {
        tag.set_year(year);
    }
    tag.write_to_path(path, Version::Id3v23)
        .map_err(|error| Error::from(format!("写入 MP3 封面失败：{error}")))?;

    let check = Tag::read_from_path(path)
        .map_err(|error| Error::from(format!("MP3 封面写入后无法校验：{error}")))?;
    if !check
        .pictures()
        .any(|picture| picture.picture_type == PictureType::CoverFront && !picture.data.is_empty())
    {
        return Err(Error::from("MP3 封面写入校验失败：未找到正面封面"));
    }
    Ok(())
}

/// 将 LRC 的逐行时间戳剥离，保留给 Apple Music 显示的文字内容。
fn lrc_to_plain_text(lyric: &str) -> String {
    lyric
        .lines()
        .filter_map(|line| {
            let mut content = line.trim();
            while let Some(rest) = content.strip_prefix('[') {
                let Some(end) = rest.find(']') else {
                    break;
                };
                let marker = &rest[..end];
                if marker.chars().next().is_some_and(char::is_numeric) {
                    content = rest[end + 1..].trim_start();
                } else {
                    break;
                }
            }
            (!content.is_empty()).then_some(content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 将歌词写入 MP3 的 ID3 USLT 帧，供 Apple Music 的歌词面板读取。
pub fn embed_lyrics_into_mp3(path: &Path, lyric: &str) -> Result<()> {
    let text = lrc_to_plain_text(lyric);
    if text.is_empty() {
        return Err(Error::from("歌词内容为空，未写入 MP3"));
    }
    let mut tag = match Tag::read_from_path(path) {
        Ok(tag) => tag,
        Err(id3::Error {
            kind: id3::ErrorKind::NoTag,
            ..
        }) => Tag::new(),
        Err(error) => return Err(Error::from(format!("读取 MP3 标签失败：{error}"))),
    };
    tag.remove("USLT");
    tag.add_frame(Lyrics {
        // 使用标准三字母码以兼容 ID3v2.3；正文按 Unicode 写入，不影响中文歌词。
        lang: "eng".to_owned(),
        description: String::new(),
        text,
    });
    tag.write_to_path(path, Version::Id3v23)
        .map_err(|error| Error::from(format!("写入 MP3 歌词失败：{error}")))?;
    let check = Tag::read_from_path(path)
        .map_err(|error| Error::from(format!("MP3 歌词写入后无法校验：{error}")))?;
    if !check.lyrics().any(|item| !item.text.trim().is_empty()) {
        return Err(Error::from("MP3 歌词写入校验失败：未找到歌词"));
    }
    Ok(())
}

/// 解析标准 LRC 的 `[mm:ss.xx]` 时间戳为毫秒。元信息标签（如 `[ar:]`）不会被当作歌词。
fn parse_lrc_timestamp(marker: &str) -> Option<u32> {
    let (minutes, seconds) = marker.split_once(':')?;
    let minutes = minutes.parse::<u32>().ok()?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let seconds = seconds.parse::<u32>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let fraction = fraction.chars().take(3).collect::<String>();
    if !fraction.is_empty() && !fraction.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let decimals = fraction.len();
    let fraction = fraction.parse::<u32>().unwrap_or(0);
    let milliseconds = match decimals {
        0 => 0,
        1 => fraction * 100,
        2 => fraction * 10,
        _ => fraction,
    };
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds.checked_mul(1_000)?)
        .and_then(|value| value.checked_add(milliseconds))
}

/// 从 LRC 取出逐行的时间轴。一个歌词行有多个时间戳时会保留每一个时间点。
fn lrc_to_timed_lines(lyric: &str) -> Vec<(u32, String)> {
    let mut lines = Vec::new();
    for line in lyric.lines() {
        let mut rest = line.trim();
        let mut timestamps = Vec::new();
        while let Some(after_open) = rest.strip_prefix('[') {
            let Some(end) = after_open.find(']') else {
                break;
            };
            let marker = &after_open[..end];
            let Some(timestamp) = parse_lrc_timestamp(marker) else {
                break;
            };
            timestamps.push(timestamp);
            rest = after_open[end + 1..].trim_start();
        }
        if rest.is_empty() {
            continue;
        }
        lines.extend(
            timestamps
                .into_iter()
                .map(|timestamp| (timestamp, rest.to_owned())),
        );
    }
    lines.sort_by_key(|(timestamp, _)| *timestamp);
    lines
}

/// 将带时间轴的 LRC 写入 MP3 的 ID3 SYLT 帧。
///
/// 这是一个兼容性增强项：部分车机和播放器可用它滚动歌词，但许多设备只认同名 `.lrc`，
/// 因此始终保留外部 LRC，且由界面默认关闭该选项。
pub fn embed_synced_lyrics_into_mp3(path: &Path, lyric: &str) -> Result<()> {
    let content = lrc_to_timed_lines(lyric);
    if content.is_empty() {
        return Err(Error::from("未找到带时间轴的 LRC 歌词，未写入同步歌词"));
    }
    let mut tag = match Tag::read_from_path(path) {
        Ok(tag) => tag,
        Err(id3::Error {
            kind: id3::ErrorKind::NoTag,
            ..
        }) => Tag::new(),
        Err(error) => return Err(Error::from(format!("读取 MP3 标签失败：{error}"))),
    };
    tag.remove("SYLT");
    tag.add_frame(SynchronisedLyrics {
        lang: "eng".to_owned(),
        timestamp_format: TimestampFormat::Ms,
        content_type: SynchronisedLyricsType::Lyrics,
        description: String::new(),
        content,
    });
    tag.write_to_path(path, Version::Id3v23)
        .map_err(|error| Error::from(format!("写入 MP3 同步歌词失败：{error}")))?;
    let check = Tag::read_from_path(path)
        .map_err(|error| Error::from(format!("MP3 同步歌词写入后无法校验：{error}")))?;
    if !check
        .synchronised_lyrics()
        .any(|item| item.timestamp_format == TimestampFormat::Ms && !item.content.is_empty())
    {
        return Err(Error::from("MP3 同步歌词写入校验失败：未找到时间轴"));
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|x| x.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// 写歌词文件，返回写入路径。
pub fn write_lyric(output: &Path, directory: &Path, lyric: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let stem = output
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("lyric");
    let target = directory.join(format!("{stem}.lrc"));
    let mut text = lyric.replace("\r\n", "\n").trim_end().to_owned();
    text.push('\n');
    std::fs::write(&target, text)?;
    Ok(target)
}

#[cfg(target_os = "macos")]
mod finder {
    use super::*;

    fn tool_exists(name: &str) -> bool {
        std::process::Command::new(name)
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok()
    }

    /// 设置访达自定义图标。需要 macOS 自带的 Rez / DeRez / SetFile（随命令行工具提供）。
    pub fn set_icon(path: &Path, image: &[u8]) -> Result<()> {
        for tool in ["sips", "DeRez", "Rez", "SetFile"] {
            if !tool_exists(tool) {
                return Err(Error::from(format!(
                    "系统缺少 {tool}，跳过访达图标（安装 Xcode 命令行工具后可用）"
                )));
            }
        }
        let work = std::env::temp_dir().join(format!("qmunlock-icon-{}", std::process::id()));
        std::fs::create_dir_all(&work)?;
        let source = work.join("icon_source.jpg");
        let rez = work.join("icon.rez");
        std::fs::write(&source, image)?;

        let output = std::process::Command::new("sips")
            .arg("-i")
            .arg(&source)
            .output()?;
        if !output.status.success() {
            return Err(Error::from("sips 生成图标资源失败"));
        }
        let derez = std::process::Command::new("DeRez")
            .args(["-only", "icns"])
            .arg(&source)
            .output()?;
        if !derez.status.success() || !derez.stdout.windows(4).any(|w| w == b"icns") {
            return Err(Error::from("DeRez 导出图标资源失败"));
        }
        std::fs::write(&rez, &derez.stdout)?;

        let append = std::process::Command::new("Rez")
            .arg("-append")
            .arg(&rez)
            .arg("-o")
            .arg(path)
            .output()?;
        if !append.status.success() {
            return Err(Error::from(format!(
                "Rez 写入资源分支失败：{}",
                String::from_utf8_lossy(&append.stderr).trim()
            )));
        }
        let flag = std::process::Command::new("SetFile")
            .args(["-a", "C"])
            .arg(path)
            .output()?;
        if !flag.status.success() {
            return Err(Error::from("SetFile 设置自定义图标标志失败"));
        }
        let _ = std::fs::remove_dir_all(&work);
        Ok(())
    }
}

/// macOS：设置访达图标；其他平台：不做任何事（保持跨平台一致）。
pub fn set_finder_icon(path: &Path, image: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        finder::set_icon(path, image)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, image);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个最小的 FLAC 结构：STREAMINFO + VORBIS_COMMENT + PADDING + 伪音频区。
    fn write_sample(path: &Path) {
        let mut data = Vec::new();
        data.extend_from_slice(b"fLaC");
        data.push(0x00); // STREAMINFO，非最后一块
        data.extend_from_slice(&[0x00, 0x00, 34]);
        data.extend_from_slice(&[0u8; 34]);
        data.push(BLOCK_VORBIS_COMMENT);
        data.extend_from_slice(&[0x00, 0x00, 4]);
        data.extend_from_slice(b"a=bc");
        data.push(0x81); // PADDING，最后一块
        data.extend_from_slice(&[0x00, 0x00, 2]);
        data.extend_from_slice(&[0u8, 0u8]);
        data.extend_from_slice(b"AUDIODATA");
        std::fs::write(path, &data).unwrap();
    }

    fn tiny_jpeg() -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        // SOF0：长度 17，精度 8，高 16，宽 32，3 个分量
        data.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20, 0x03, 0x01, 0x11, 0x00, 0x02,
            0x11, 0x00, 0x03, 0x11, 0x00,
        ]);
        data.extend_from_slice(&[0xFF, 0xD9]);
        data
    }

    #[test]
    fn embeds_mp3_front_cover_and_replaces_existing_one() {
        let dir = std::env::temp_dir().join(format!("qmunlock-tags-mp3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.mp3");
        // id3 只处理标签区；这里不需要真实音频来验证 APIC 的写入和回读。
        std::fs::write(&file, b"MP3-AUDIO-DATA").unwrap();
        let meta = SongMeta {
            album_mid: "album-mid".into(),
            album: "测试专辑".into(),
            album_singers: "测试专辑艺人".into(),
            title: "测试歌曲".into(),
            singers: "测试歌手".into(),
            release_date: "2024-05-01".into(),
            track_number: Some(3),
            disc_number: Some(2),
            song_mid: "song-mid".into(),
        };

        embed_cover_into_mp3(&file, &tiny_jpeg(), "第一张", &meta).unwrap();
        embed_cover_into_mp3(&file, &tiny_jpeg(), "替换后的封面", &meta).unwrap();

        let tag = Tag::read_from_path(&file).unwrap();
        let covers: Vec<_> = tag
            .pictures()
            .filter(|picture| picture.picture_type == PictureType::CoverFront)
            .collect();
        assert_eq!(covers.len(), 1, "正面封面应替换而不是叠加");
        assert_eq!(covers[0].description, "替换后的封面");
        assert!(!covers[0].data.is_empty());
        assert_eq!(tag.title(), Some("测试歌曲"));
        assert_eq!(tag.artist(), Some("测试歌手"));
        assert_eq!(tag.album(), Some("测试专辑"));
        assert_eq!(tag.album_artist(), Some("测试专辑艺人"));
        assert_eq!(tag.track(), Some(3));
        assert_eq!(tag.disc(), Some(2));
        assert_eq!(tag.year(), Some(2024));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn embeds_plain_lyrics_into_mp3() {
        let dir = std::env::temp_dir().join(format!("qmunlock-lyrics-mp3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.mp3");
        std::fs::write(&file, b"MP3-AUDIO-DATA").unwrap();

        embed_lyrics_into_mp3(&file, "[00:01.00]第一行\n[00:02.50][00:03.00]第二行").unwrap();

        let tag = Tag::read_from_path(&file).unwrap();
        assert_eq!(
            tag.lyrics().next().map(|item| item.text.as_str()),
            Some("第一行\n第二行")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn embeds_timed_lyrics_into_mp3() {
        let dir = std::env::temp_dir().join(format!("qmunlock-sylt-mp3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.mp3");
        std::fs::write(&file, b"MP3-AUDIO-DATA").unwrap();

        embed_synced_lyrics_into_mp3(
            &file,
            "[ar:测试歌手]\n[00:01.05]第一行\n[00:02.500][00:03.00]第二行",
        )
        .unwrap();

        let tag = Tag::read_from_path(&file).unwrap();
        let timed = tag.synchronised_lyrics().next().unwrap();
        assert_eq!(timed.timestamp_format, TimestampFormat::Ms);
        assert_eq!(
            timed.content,
            vec![
                (1_050, "第一行".to_owned()),
                (2_500, "第二行".to_owned()),
                (3_000, "第二行".to_owned()),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jpeg_size_parsing() {
        assert_eq!(jpeg_dimensions(&tiny_jpeg()), (32, 16));
    }

    #[test]
    fn embeds_cover_and_keeps_audio() {
        let dir = std::env::temp_dir().join(format!("qmunlock-tags-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.flac");
        write_sample(&file);

        let jpeg = tiny_jpeg();
        embed_cover_into_flac(&file, &jpeg, "专辑").unwrap();

        let mut handle = std::fs::File::open(&file).unwrap();
        let blocks = read_blocks(&mut handle).unwrap();
        let pictures: Vec<_> = blocks
            .iter()
            .filter(|(kind, _)| *kind == BLOCK_PICTURE)
            .collect();
        assert_eq!(pictures.len(), 1, "应当只有一张封面");
        let (_, picture) = pictures[0];
        assert!(picture.len() > jpeg.len(), "PICTURE 块应包含图片与头部字段");

        let audio_length =
            std::fs::metadata(&file).unwrap().len() - handle.stream_position().unwrap();
        assert_eq!(audio_length, b"AUDIODATA".len() as u64, "音频区不能变动");

        // 再写一次应当替换而不是叠加
        embed_cover_into_flac(&file, &jpeg, "专辑").unwrap();
        let mut again = std::fs::File::open(&file).unwrap();
        let blocks = read_blocks(&mut again).unwrap();
        assert_eq!(
            blocks
                .iter()
                .filter(|(kind, _)| *kind == BLOCK_PICTURE)
                .count(),
            1
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 造一个指定类型的 PICTURE 块数据：类型 + mime + 描述 + 尺寸字段 + 图片字节。
    fn picture_block(kind: u32) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&kind.to_be_bytes());
        let mime = b"image/jpeg";
        data.extend_from_slice(&(mime.len() as u32).to_be_bytes());
        data.extend_from_slice(mime);
        let desc = b"cover";
        data.extend_from_slice(&(desc.len() as u32).to_be_bytes());
        data.extend_from_slice(desc);
        data.extend_from_slice(&[0u8; 16]); // 宽 / 高 / 位深 / 色数
        let body = tiny_jpeg();
        data.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(&body);
        data
    }

    /// 造一个带 Vorbis 标签、且同时含封底与正面封面两张图的 FLAC。
    fn write_tagged_sample(path: &Path) {
        let comments = ["TITLE=GLORIA", "ARTIST=G.E.M.邓紫棋", "ALBUM=启示录"];
        let mut comment = Vec::new();
        let vendor = b"test";
        comment.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        comment.extend_from_slice(vendor);
        comment.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for entry in comments {
            comment.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            comment.extend_from_slice(entry.as_bytes());
        }

        let mut data = Vec::new();
        data.extend_from_slice(b"fLaC");
        data.push(0x00); // STREAMINFO
        data.extend_from_slice(&[0x00, 0x00, 34]);
        data.extend_from_slice(&[0u8; 34]);
        data.push(BLOCK_VORBIS_COMMENT);
        data.extend_from_slice(&((comment.len() as u32).to_be_bytes()[1..]));
        data.extend_from_slice(&comment);
        data.push(BLOCK_PICTURE); // 封底（类型 4），非最后一块
        let back = picture_block(4);
        data.extend_from_slice(&((back.len() as u32).to_be_bytes()[1..]));
        data.extend_from_slice(&back);
        data.push(0x80 | BLOCK_PICTURE); // 正面封面（类型 3），最后一块
        let front = picture_block(3);
        data.extend_from_slice(&((front.len() as u32).to_be_bytes()[1..]));
        data.extend_from_slice(&front);
        data.extend_from_slice(b"AUDIODATA");
        std::fs::write(path, &data).unwrap();
    }

    #[test]
    fn replaces_only_front_cover_and_keeps_others() {
        let dir = std::env::temp_dir().join(format!("qmunlock-tags-front-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tagged.flac");
        write_tagged_sample(&file);

        embed_cover_into_flac(&file, &tiny_jpeg(), "新封面").unwrap();

        let mut handle = std::fs::File::open(&file).unwrap();
        let blocks = read_blocks(&mut handle).unwrap();
        let fronts = blocks
            .iter()
            .filter(|block| is_front_picture(block))
            .count();
        let others = blocks
            .iter()
            .filter(|block| block.0 == BLOCK_PICTURE && !is_front_picture(block))
            .count();
        assert_eq!(fronts, 1, "正面封面应只有一张");
        assert_eq!(others, 1, "封底等其他内嵌图片必须保留");
        let audio = std::fs::metadata(&file).unwrap().len() - handle.stream_position().unwrap();
        assert_eq!(audio, b"AUDIODATA".len() as u64, "音频区不能变动");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_flac_tags_and_validates_search_result() {
        let dir = std::env::temp_dir().join(format!("qmunlock-tags-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tagged.flac");
        write_tagged_sample(&file);

        let found = read_embedded_tags(&file).expect("应读到 Vorbis 标签");
        assert_eq!(found.title, "GLORIA");
        assert_eq!(found.artist, "G.E.M.邓紫棋");
        assert_eq!(found.album, "启示录");

        let meta = SongMeta {
            album_mid: "mid".into(),
            album: "启示录".into(),
            album_singers: "G.E.M.邓紫棋".into(),
            title: "GLORIA".into(),
            singers: "G.E.M.邓紫棋".into(),
            release_date: "2022-09-23".into(),
            track_number: Some(1),
            disc_number: Some(1),
            song_mid: "mid".into(),
        };
        assert!(matches_embedded_tags(&meta, &found), "同名同歌手应匹配");

        let live = SongMeta {
            title: "GLORIA (Live)".into(),
            ..meta.clone()
        };
        assert!(matches_embedded_tags(&live, &found), "后缀差异应容忍");

        let wrong_title = SongMeta {
            title: "泡沫".into(),
            ..meta.clone()
        };
        assert!(
            !matches_embedded_tags(&wrong_title, &found),
            "标题不同应不匹配"
        );

        let wrong_singer = SongMeta {
            singers: "其他人".into(),
            ..meta.clone()
        };
        assert!(
            !matches_embedded_tags(&wrong_singer, &found),
            "歌手不同应不匹配"
        );

        assert!(
            matches_embedded_tags(&wrong_title, &EmbeddedTags::default()),
            "文件没有标签时维持原有按文件名检索的行为"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 造一个最小 ID3v2.3 标签：两个无关帧（APIC/COMM）排在文本帧前面，
    /// 标签区结束后再拼一段「假帧」，验证解析既会跳帧、也停在声明长度处。
    fn write_id3_sample(path: &Path) {
        fn synchsafe_bytes(value: usize) -> [u8; 4] {
            [
                (value >> 21) as u8 & 0x7F,
                (value >> 14) as u8 & 0x7F,
                (value >> 7) as u8 & 0x7F,
                value as u8 & 0x7F,
            ]
        }
        fn frame(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(id);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&[0u8; 2]);
            out.extend_from_slice(body);
            out
        }
        fn text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
            let mut body = vec![0u8];
            body.extend_from_slice(text.as_bytes());
            frame(id, &body)
        }
        let mut body = Vec::new();
        body.extend_from_slice(&frame(b"APIC", b"DECOYDATA"));
        body.extend_from_slice(&frame(b"COMM", b"DECOYDATA"));
        body.extend_from_slice(&text_frame(b"TIT2", "GLORIA"));
        body.extend_from_slice(&text_frame(b"TPE1", "G.E.M.邓紫棋"));
        body.extend_from_slice(&text_frame(b"TALB", "启示录"));
        let mut file = Vec::new();
        file.extend_from_slice(b"ID3");
        file.extend_from_slice(&[3, 0, 0]);
        file.extend_from_slice(&synchsafe_bytes(body.len()));
        file.extend_from_slice(&body);
        // 标签区之外的假 TIT2 帧：不应被读进标题
        file.extend_from_slice(b"TIT2");
        file.extend_from_slice(&[0xFF; 16]);
        std::fs::write(path, &file).unwrap();
    }

    #[test]
    fn id3_tags_skip_decoy_frames_and_stop_at_tag_end() {
        let dir = std::env::temp_dir().join(format!("qmunlock-tags-id3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tagged.mp3");
        write_id3_sample(&file);

        let found = read_embedded_tags(&file).expect("应读到 ID3v2 标签");
        assert_eq!(found.title, "GLORIA", "无关帧后面的标题帧不能漏");
        assert_eq!(found.artist, "G.E.M.邓紫棋");
        assert_eq!(found.album, "启示录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pick_walks_candidates_and_compares_album() {
        let tags = EmbeddedTags {
            title: "GLORIA".into(),
            artist: "G.E.M.邓紫棋".into(),
            album: "启示录".into(),
        };
        let wrong_title = SongMeta {
            album_mid: "a".into(),
            album: "启示录".into(),
            album_singers: "G.E.M.邓紫棋".into(),
            title: "泡沫".into(),
            singers: "G.E.M.邓紫棋".into(),
            release_date: "2022-09-23".into(),
            track_number: Some(1),
            disc_number: Some(1),
            song_mid: "a".into(),
        };
        let right = SongMeta {
            album_mid: "b".into(),
            title: "GLORIA".into(),
            ..wrong_title.clone()
        };
        let picked = pick_matching_meta("GLORIA", vec![wrong_title.clone(), right], Some(&tags))
            .expect("应跳过第一条不符候选、选中后面匹配的一条");
        assert_eq!(picked.album_mid, "b");

        let other_album = SongMeta {
            album: "新的心跳".into(),
            title: "GLORIA".into(),
            ..wrong_title.clone()
        };
        assert!(
            pick_matching_meta("GLORIA", vec![other_album], Some(&tags)).is_err(),
            "同名同歌手但专辑不同应判为不符"
        );

        let first = pick_matching_meta(
            "GLORIA",
            vec![
                wrong_title,
                SongMeta {
                    album_mid: "b".into(),
                    album: "启示录".into(),
                    album_singers: "G.E.M.邓紫棋".into(),
                    title: "GLORIA".into(),
                    singers: "G.E.M.邓紫棋".into(),
                    release_date: "2022-09-23".into(),
                    track_number: Some(1),
                    disc_number: Some(1),
                    song_mid: "b".into(),
                },
            ],
            None,
        )
        .expect("无标签时取第一个候选");
        assert_eq!(first.album_mid, "a", "无标签应维持取第一条的原行为");
    }
}
