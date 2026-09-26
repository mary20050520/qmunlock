use super::{Credentials, Error, MusicExFooter, Result};
use serde_json::json;

const API_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";

pub async fn fetch(
    footer: &MusicExFooter,
    credentials: &Credentials,
    platform: &str,
) -> Result<String> {
    let payload = json!({
        "comm": { "authst": credentials.authst, "ct": "19", "cv": "1859", "uin": credentials.uin, "tmeLoginType": credentials.login_type },
        "req_1": { "module": "music.vkey.GetEVkey", "method": "CgiGetEVkey", "param": { "filename": [footer.filename], "guid": "10000", "songmid": [footer.song_mid], "songtype": [1], "uin": credentials.uin, "loginflag": 1, "platform": platform, "ctx": 1 } }
    });
    let response: serde_json::Value = reqwest::Client::new()
        .post(API_URL)
        .header("User-Agent", "QQMusic/20 QMUnlock")
        .json(&payload)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let result = response
        .pointer("/req_1/data/midurlinfo/0/ekey")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty());
    result.map(str::to_owned).ok_or_else(|| {
        let request_code = response
            .pointer("/req_1/code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let item_code = response
            .pointer("/req_1/data/retcode")
            .or_else(|| response.pointer("/req_1/data/midurlinfo/0/errcode"))
            .and_then(|v| v.as_i64())
            .map(|v| format!("，曲目状态 {v}"))
            .unwrap_or_default();
        Error::from(format!(
            "QQ 音乐未返回 ekey（请求状态 {request_code}{item_code}）。读取到的登录凭据可能已过期，或无法访问 QQ 音乐的最新登录信息；请重新登录 QQ 音乐后重试。若仍失败，请在「系统设置 → 隐私与安全性 → 完全磁盘访问权限」中允许 QM Unlock，然后重新打开应用。"
        ))
    })
}
