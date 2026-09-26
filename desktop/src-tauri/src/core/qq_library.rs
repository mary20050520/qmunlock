//! 把解密后的文件链接回 QQ 音乐本地库。
//!
//! 背景：QQ 音乐在 `qqmusic.sqlite` 的 `SONGS` 表里维护本地文件记录。只有
//! `id` 等于曲库当前歌曲 id、`type=13` 且带文件路径的记录，客户端才会把它当作
//! 「已下载的在线歌曲」（收藏 / 评论区可用）。解密替换掉 .mflac 之后，文件往往留下
//! 一条 id 为旧条目的记录，于是降级成普通本地文件。
//!
//! 这里做的事等价于客户端自己的「匹配在线歌曲」：把文件路径挂到正确的曲库条目上，
//! 并把歌曲加入「下载成功」列表。
//!
//! 这是 macOS 独有的集成（依赖 QQ 音乐 Mac 客户端的数据布局），其他平台全部为空操作。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub file: String,
    pub song_id: u64,
    pub title: String,
    pub singer: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_mid: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub pending: usize,
    pub database_found: bool,
    pub app_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 写入 QQ 音乐本地库；客户端未退出时记入待办，返回给用户看的说明。
pub fn link_or_queue(entry: &LibraryEntry) -> String {
    #[cfg(target_os = "macos")]
    {
        match mac::apply(entry) {
            Ok(note) => note,
            Err(error) => {
                let queued = mac::queue(entry).is_ok();
                if queued {
                    format!("已记下，稍后补做 QQ 音乐链接（{error}）")
                } else {
                    format!("QQ 音乐链接未完成：{error}")
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = entry;
        String::new()
    }
}

/// 补做所有待办的链接（客户端未退出时保持待办不动）。
pub fn flush_pending() -> String {
    #[cfg(target_os = "macos")]
    {
        mac::flush()
    }
    #[cfg(not(target_os = "macos"))]
    {
        String::new()
    }
}

pub fn status() -> LibraryStatus {
    #[cfg(target_os = "macos")]
    {
        mac::status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        LibraryStatus {
            pending: 0,
            database_found: false,
            app_running: false,
            message: None,
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::core::{Error, Result};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 「下载成功」列表在 NEWFOLDERS 里的 seq
    const DOWNLOAD_SEQ: i64 = 3;
    /// 曲库歌曲在 SONGS 里的 type
    const SONG_TYPE: i64 = 13;
    const NEED_SYN: i64 = 422_004_125;
    const OP_TYPE: i64 = 7;
    const SQLITE: &str = "/usr/bin/sqlite3";

    fn app_data_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
        PathBuf::from(home).join("Library/Application Support/qmunlock")
    }

    fn queue_file() -> PathBuf {
        app_data_dir().join("pending-library.json")
    }

    pub fn database_path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let candidates = [
            format!("{home}/Library/Containers/com.tencent.QQMusicMac/Data/Library/Application Support/QQMusicMac/qqmusic.sqlite"),
            format!("{home}/Library/Application Support/QQMusicMac/qqmusic.sqlite"),
        ];
        candidates
            .iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
    }

    pub fn app_running() -> bool {
        std::process::Command::new("/usr/bin/pgrep")
            .args(["-x", "QQMusic"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn escape(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn run_sql(db: &Path, sql: &str) -> Result<String> {
        let mut child = std::process::Command::new(SQLITE)
            .arg(db)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| Error::from("无法写入 sqlite3 进程"))?
            .write_all(sql.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(Error::from(format!(
                "sqlite3 执行失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn columns(db: &Path, table: &str) -> Vec<String> {
        run_sql(db, &format!("PRAGMA table_info({table});"))
            .map(|text| {
                text.lines()
                    .filter_map(|line| line.split('|').nth(1).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 同一秒内多次备份时追加序号，避免后一次覆盖前一次的副本。
    fn unique_backup_dir(root: &Path, stamp: u64) -> PathBuf {
        let base = root.join(stamp.to_string());
        let mut dir = base;
        let mut index = 1u32;
        while dir.exists() {
            dir = root.join(format!("{stamp}-{index}"));
            index += 1;
        }
        dir
    }

    fn backup(db: &Path) -> Result<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or_default();
        let dir = unique_backup_dir(&app_data_dir().join("qq-library-backups"), stamp);
        std::fs::create_dir_all(&dir)?;
        for suffix in ["", "-wal", "-shm"] {
            let source = PathBuf::from(format!("{}{suffix}", db.display()));
            if source.exists() {
                let name = source
                    .file_name()
                    .map(|value| value.to_os_string())
                    .unwrap_or_default();
                std::fs::copy(&source, dir.join(name))?;
            }
        }
        Ok(dir)
    }

    fn build_sql(db: &Path, entry: &LibraryEntry) -> Result<String> {
        let path = Path::new(&entry.file);
        if !path.exists() {
            return Err(Error::from("文件不存在，无法链接"));
        }
        let size = std::fs::metadata(path)?.len();
        let song_columns = columns(db, "SONGS");
        let folder_columns = columns(db, "NEWFOLDERSONGS");
        // 旧记录只清理「与当前文件明确对应」的：同一路径的重复行，或同目录同名的
        // 加密源（mgg / mmp4）。不再按歌名 + 歌手前缀删，避免误删同名歌曲的其他版本。
        let dir = path
            .parent()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let stale_mgg = format!("{dir}/{stem}.mgg");
        let stale_mmp4 = format!("{dir}/{stem}.mmp4");

        let mut insert_columns = vec!["id", "type", "name", "singer"];
        let mut insert_values = vec![
            entry.song_id.to_string(),
            SONG_TYPE.to_string(),
            format!("'{}'", escape(&entry.title)),
            format!("'{}'", escape(&entry.singer)),
        ];
        for (name, value) in [
            ("album", format!("'{}'", escape(&entry.album))),
            ("file", format!("'{}'", escape(&entry.file))),
            ("filesize", size.to_string()),
            ("K_SONG_RESERVE9", format!("'{}'", escape(&entry.album_mid))),
            ("K_SONG_RESERVEINT23", size.to_string()),
        ] {
            if song_columns.iter().any(|column| column == name) {
                insert_columns.push(name);
                insert_values.push(value);
            }
        }

        let mut folder_columns_sql = vec!["seq", "id", "type", "addtime", "opType"];
        let mut folder_values = vec![
            DOWNLOAD_SEQ.to_string(),
            entry.song_id.to_string(),
            SONG_TYPE.to_string(),
            "strftime('%s','now')".to_owned(),
            OP_TYPE.to_string(),
        ];
        if folder_columns.iter().any(|column| column == "needSyn") {
            folder_columns_sql.push("needSyn");
            folder_values.push(NEED_SYN.to_string());
        }

        Ok(format!(
            "BEGIN;\n\
             INSERT INTO SONGS ({insert_columns})\n\
             SELECT {insert_values}\n\
             WHERE NOT EXISTS (SELECT 1 FROM SONGS WHERE id={id} AND type={SONG_TYPE});\n\
             UPDATE SONGS SET file='{file}', filesize={size}{extra}\n\
             WHERE id={id} AND type={SONG_TYPE};\n\
             DELETE FROM SONGS WHERE file<>'' AND NOT (id={id} AND type={SONG_TYPE})\n\
             AND (file='{file}' OR file='{mgg}' OR file='{mmp4}');\n\
             INSERT INTO NEWFOLDERSONGS ({folder_insert})\n\
             SELECT {folder_select}\n\
             WHERE NOT EXISTS (SELECT 1 FROM NEWFOLDERSONGS WHERE seq={DOWNLOAD_SEQ} AND id={id});\n\
             UPDATE NEWFOLDERS SET foldercount=(SELECT count(*) FROM NEWFOLDERSONGS WHERE seq={DOWNLOAD_SEQ})\n\
             WHERE seq={DOWNLOAD_SEQ};\n\
             COMMIT;\n",
            insert_columns = insert_columns.join(", "),
            insert_values = insert_values.join(", "),
            id = entry.song_id,
            file = escape(&entry.file),
            size = size,
            extra = if song_columns.iter().any(|column| column == "K_SONG_RESERVEINT23") {
                format!(", K_SONG_RESERVEINT23={size}")
            } else {
                String::new()
            },
            mgg = escape(&stale_mgg),
            mmp4 = escape(&stale_mmp4),
            folder_insert = folder_columns_sql.join(", "),
            folder_select = folder_values.join(", "),
        ))
    }

    pub fn apply(entry: &LibraryEntry) -> Result<String> {
        if app_running() {
            return Err(Error::from("QQ 音乐正在运行，需完全退出后才能写入其数据库"));
        }
        let db = database_path().ok_or_else(|| Error::from("没找到 QQ 音乐的数据库"))?;
        let sql = build_sql(&db, entry)?;
        backup(&db)?;
        run_sql(&db, &sql)?;
        let check = run_sql(
            &db,
            &format!(
                "SELECT count(*) FROM SONGS WHERE id={} AND type={SONG_TYPE} AND file<>'';",
                entry.song_id
            ),
        )?;
        if check.trim() == "0" {
            return Err(Error::from("写入后校验未通过"));
        }
        Ok("已链接到 QQ 音乐库（含「下载成功」列表）".to_owned())
    }

    pub fn read_queue() -> Vec<LibraryEntry> {
        std::fs::read_to_string(queue_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_queue(entries: &[LibraryEntry]) -> Result<()> {
        std::fs::create_dir_all(app_data_dir())?;
        let text = serde_json::to_string_pretty(entries)
            .map_err(|error| Error::from(format!("待办列表写入失败：{error}")))?;
        std::fs::write(queue_file(), text)?;
        Ok(())
    }

    pub fn queue(entry: &LibraryEntry) -> Result<()> {
        let mut entries = read_queue();
        if entries.iter().any(|item| item.file == entry.file) {
            return Ok(());
        }
        entries.push(entry.clone());
        write_queue(&entries)
    }

    pub fn flush() -> String {
        let entries = read_queue();
        if entries.is_empty() {
            return String::new();
        }
        if app_running() {
            return format!("还有 {} 首等待链接，请退出 QQ 音乐后再试", entries.len());
        }
        if database_path().is_none() {
            return "没找到 QQ 音乐的数据库，暂时无法补做".to_owned();
        }
        let mut done = 0usize;
        let mut left = Vec::new();
        let mut last_error = String::new();
        for entry in entries {
            match apply(&entry) {
                Ok(_) => done += 1,
                Err(error) => {
                    last_error = error.to_string();
                    left.push(entry);
                }
            }
        }
        let _ = write_queue(&left);
        if left.is_empty() {
            format!("已补做 {done} 首的 QQ 音乐链接")
        } else {
            format!(
                "已补做 {done} 首，仍有 {} 首未完成（{last_error}）",
                left.len()
            )
        }
    }

    pub fn status() -> LibraryStatus {
        let entries = read_queue();
        LibraryStatus {
            pending: entries.len(),
            database_found: database_path().is_some(),
            app_running: app_running(),
            message: None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn backup_dirs_never_collide() {
            let root =
                std::env::temp_dir().join(format!("qmunlock-lib-backup-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("1700000000")).unwrap();

            let first = unique_backup_dir(&root, 1700000000);
            assert_eq!(first.file_name().unwrap(), "1700000000-1");
            std::fs::create_dir_all(&first).unwrap();
            let second = unique_backup_dir(&root, 1700000000);
            assert_eq!(
                second.file_name().unwrap(),
                "1700000000-2",
                "同一秒内不能复用目录"
            );
            let fresh = unique_backup_dir(&root, 1700000001);
            assert_eq!(fresh.file_name().unwrap(), "1700000001");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn delete_predicate_only_touches_matching_files() {
            let dir = std::env::temp_dir().join(format!("qmunlock-lib-sql-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("qqmusic.sqlite");
            let audio = dir.join("song.flac");
            std::fs::write(&audio, b"fake").unwrap();
            let created = std::process::Command::new("sqlite3")
                .arg(&db)
                .arg(
                    "CREATE TABLE SONGS(id INTEGER, type INTEGER, name TEXT, singer TEXT, \
                     album TEXT, file TEXT, filesize INTEGER, K_SONG_RESERVE9 TEXT, \
                     K_SONG_RESERVEINT23 INTEGER); \
                     CREATE TABLE NEWFOLDERSONGS(seq INTEGER, id INTEGER, type INTEGER, \
                     addtime TEXT, opType INTEGER, needSyn INTEGER); \
                     CREATE TABLE NEWFOLDERS(seq INTEGER, foldercount INTEGER);",
                )
                .status()
                .unwrap();
            assert!(created.success(), "测试库创建失败");

            let entry = LibraryEntry {
                file: audio.display().to_string(),
                song_id: 42,
                title: "同名歌".into(),
                singer: "某歌手".into(),
                album: "某专辑".into(),
                album_mid: "mid".into(),
            };
            let sql = build_sql(&db, &entry).unwrap();
            assert!(!sql.contains("singer LIKE"), "不应再按歌手前缀删除：{sql}");
            assert!(!sql.contains("name='同名歌' AND file"), "不应再按歌名删除");
            let mgg = dir.join("song.mgg").display().to_string();
            assert!(
                sql.contains(&format!("file='{}'", escape(&mgg))),
                "应清理同目录同名加密源：{sql}"
            );
            assert!(
                sql.contains(&format!("file='{}'", escape(&entry.file))),
                "应清理同路径重复行：{sql}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
