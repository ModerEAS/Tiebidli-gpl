//! 主题音乐 Action：`GET /action/handsome-meting-api`。
//!
//! 路径字面量与现网 LocalConst `MUSIC_API` / `MUSIC_API_PARSE` 一致（主题 Action，非引擎能力）。
//! 参数校验对齐参考 `MetingAction::check`；`do=parse` 的 URL 识别规则同 `shortcode()`。
//! 上游调用是按公开 Meting（MIT）文档自写的精简实现：目前只实现 `netease` 源，
//! 其它源回 403（与参数校验失败同码），前台胶水只配网易源即可。

use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 对齐 `MetingAction::check`（xiami 已停服，仍保留名字校验以便提示，不实际请求）。
pub fn check(server: &str, kind: &str, id: &str) -> bool {
    matches!(server, "netease" | "tencent" | "baidu" | "xiami" | "kugou")
        && matches!(
            kind,
            "song" | "album" | "search" | "artist" | "playlist" | "lrc" | "url" | "pic"
        )
        && !id.is_empty()
}

/// APlayer 曲目形状（自定，文档见 `THEME_PARITY_RESULT.md`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Track {
    pub name: String,
    pub artist: String,
    pub url: String,
    pub cover: String,
    pub lrc: String,
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok()
}

fn headers() -> Vec<(&'static str, String)> {
    vec![
        ("Referer", "https://music.163.com".to_string()),
        ("User-Agent", UA.to_string()),
    ]
}

async fn get_json(url: &str) -> Option<serde_json::Value> {
    let client = client()?;
    let mut req = client.get(url);
    for (k, v) in headers() {
        req = req.header(k, v);
    }
    let text = req.send().await.ok()?;
    if !text.status().is_success() {
        return None;
    }
    serde_json::from_str(&text.text().await.ok()?).ok()
}

fn artists_of(song: &serde_json::Value) -> String {
    song.get("ar")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

fn cover_of(song: &serde_json::Value, size: u32) -> String {
    let pic = song
        .get("al")
        .and_then(|al| al.get("picUrl"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    if pic.is_empty() {
        String::new()
    } else {
        format!("{pic}?param={size}y{size}")
    }
}

async fn song_url(id: &str) -> String {
    let url = format!("https://music.163.com/api/song/enhance/player/url?ids=[{id}]&br=320");
    get_json(&url)
        .await
        .and_then(|v| {
            v.get("data")?.as_array()?.first()?.get("url")?.as_str().map(str::to_string)
        })
        .unwrap_or_default()
}

async fn song_lyric(id: &str) -> String {
    let url = format!("https://music.163.com/api/song/lyric?id={id}&lv=-1&kv=-1&tv=-1");
    get_json(&url)
        .await
        .and_then(|v| {
            v.get("lrc")?.get("lyric")?.as_str().map(str::to_string)
        })
        .unwrap_or_default()
}

async fn song_detail(id: &str) -> Option<serde_json::Value> {
    let body = format!("c=%5B%7B%22id%22%3A{id}%7D%5D");
    let url = format!("https://music.163.com/api/v3/song/detail/?{body}");
    get_json(&url)
        .await
        .and_then(|v| v.get("songs")?.as_array()?.first().cloned())
}

async fn track_of(id: &str, with_lyric: bool) -> Option<Track> {
    let song = song_detail(id).await?;
    let url = song_url(id).await;
    if url.is_empty() {
        return None;
    }
    let lrc = if with_lyric { song_lyric(id).await } else { String::new() };
    Some(Track {
        name: song.get("name")?.as_str().unwrap_or("").to_string(),
        artist: artists_of(&song),
        url,
        cover: cover_of(&song, 300),
        lrc,
    })
}

/// `type=song`。
pub async fn api_song(id: &str) -> Option<Vec<Track>> {
    track_of(id, true).await.map(|t| vec![t])
}

/// `type=url`：只取播放地址。
pub async fn api_url(id: &str) -> Option<serde_json::Value> {
    let url = song_url(id).await;
    if url.is_empty() {
        return None;
    }
    let id_num: i64 = id.parse().ok()?;
    Some(serde_json::json!({"id": id_num, "url": url}))
}

/// `type=lrc`：只取歌词。
pub async fn api_lrc(id: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({"lyric": song_lyric(id).await}))
}

/// `type=pic`：只取封面。
pub async fn api_pic(id: &str) -> Option<serde_json::Value> {
    let song = song_detail(id).await?;
    Some(serde_json::json!({"url": cover_of(&song, 300)}))
}

/// `type=search`。
pub async fn api_search(keyword: &str) -> Option<Vec<Track>> {
    let client = client()?;
    let mut req = client.post("https://music.163.com/api/cloudsearch/pc");
    for (k, v) in headers() {
        req = req.header(k, v);
    }
    let text = req
        .form(&[("s", keyword), ("type", "1"), ("limit", "30"), ("offset", "0")])
        .send()
        .await
        .ok()?;
    if !text.status().is_success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&text.text().await.ok()?).ok()?;
    let songs = v.pointer("/result/songs")?.as_array()?;
    let mut out = Vec::new();
    for song in songs.iter().take(30) {
        let id = song.get("id")?.to_string();
        let Some(mut track) = track_of(&id, false).await else { continue };
        track.name = song.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        if track.artist.is_empty() {
            track.artist = artists_of(song);
        }
        if track.cover.is_empty() {
            track.cover = cover_of(song, 300);
        }
        out.push(track);
    }
    Some(out)
}

async fn batch_tracks(ids: &[String], with_lyric: bool) -> Vec<Track> {
    let mut out = Vec::new();
    for id in ids.iter().take(100) {
        if let Some(t) = track_of(id, with_lyric).await {
            out.push(t);
        }
    }
    out
}

/// `type=playlist`。
pub async fn api_playlist(id: &str) -> Option<Vec<Track>> {
    let v = get_json(&format!("https://music.163.com/api/v6/playlist/detail?id={id}")).await?;
    let ids: Vec<String> = v
        .pointer("/playlist/trackIds")?
        .as_array()?
        .iter()
        .filter_map(|t| t.get("id").map(|n| n.to_string()))
        .collect();
    Some(batch_tracks(&ids, false).await)
}

/// `type=artist`（热门单曲）。
pub async fn api_artist(id: &str) -> Option<Vec<Track>> {
    let v = get_json(&format!("https://music.163.com/api/v1/artist/{id}")).await?;
    let ids: Vec<String> = v
        .get("hotSongs")?
        .as_array()?
        .iter()
        .take(50)
        .filter_map(|t| t.get("id").map(|n| n.to_string()))
        .collect();
    Some(batch_tracks(&ids, false).await)
}

/// `type=album`。
pub async fn api_album(id: &str) -> Option<Vec<Track>> {
    let v = get_json(&format!("https://music.163.com/api/v1/album/{id}")).await?;
    let songs = v.get("songs")?.as_array()?;
    let mut out = Vec::new();
    for song in songs {
        let id = song.get("id")?.to_string();
        let url = song_url(&id).await;
        if url.is_empty() {
            continue;
        }
        out.push(Track {
            name: song.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            artist: artists_of(song),
            url,
            cover: cover_of(song, 300),
            lrc: String::new(),
        });
    }
    Some(out)
}

/// `do=parse` 的 URL 识别（对齐 `MetingAction::shortcode`）。返回 `(server, id, type)`。
pub fn parse_music_url(url: &str) -> (String, String, String) {
    let url = url.trim();
    if url.is_empty() {
        return (String::new(), String::new(), String::new());
    }
    if url.contains("163.com") {
        let server = "netease".to_string();
        for (pat, kind) in [
            ("playlist", "playlist"),
            ("toplist", "playlist"),
            ("album", "album"),
            ("song", "song"),
            ("artist", "artist"),
        ] {
            if let Some(id) = capture_id(url, pat) {
                return (server, id, kind.to_string());
            }
        }
        return (server, String::new(), String::new());
    }
    if url.contains("qq.com") {
        for (pat, kind) in [
            ("playsquare/", "playlist"),
            ("playlist/", "playlist"),
            ("album/", "album"),
            ("songDetail/", "song"),
            ("song/", "song"),
            ("singer/", "artist"),
        ] {
            if let Some(id) = capture_seg(url, pat) {
                return ("tencent".into(), id, kind.to_string());
            }
        }
        return ("tencent".into(), String::new(), String::new());
    }
    if url.contains("kugou.com") {
        if let Some(id) = capture_hash(url) {
            return ("kugou".into(), id, "song".into());
        }
        for (pat, kind) in [("special/single/", "playlist"), ("album/", "album"), ("singer/", "artist")] {
            if let Some(id) = capture_seg(url, pat) {
                return ("kugou".into(), id, kind.to_string());
            }
        }
        return ("kugou".into(), String::new(), String::new());
    }
    if url.contains("baidu.com") {
        for (pat, kind) in [("songlist/", "playlist"), ("album/", "album"), ("song/", "song"), ("artist/", "artist")] {
            if let Some(id) = capture_seg(url, pat) {
                return ("baidu".into(), id, kind.to_string());
            }
        }
        return ("baidu".into(), String::new(), String::new());
    }
    (String::new(), String::new(), String::new())
}

fn capture_id(url: &str, kind: &str) -> Option<String> {
    let i = url.find(kind)?;
    let rest = &url[i + kind.len()..];
    let digits: String = rest
        .trim_start_matches(|c| c == '/' || c == '?' || c == '=')
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    // `?id=123` 或 `/123` 两种写法。
    if digits.is_empty() {
        let after_eq = rest.split("id=").nth(1)?;
        let digits: String = after_eq.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        return Some(digits);
    }
    Some(digits)
}

fn capture_seg(url: &str, marker: &str) -> Option<String> {
    let i = url.find(marker)?;
    let seg: String = url[i + marker.len()..]
        .chars()
        .take_while(|c| *c != '/' && *c != '.' && *c != '?' && *c != '#' && *c != '&')
        .collect();
    if seg.is_empty() { None } else { Some(seg) }
}

fn capture_hash(url: &str) -> Option<String> {
    let i = url.find("#hash=")?;
    let seg: String = url[i + 6..].chars().take_while(|c| c.is_alphanumeric()).collect();
    if seg.is_empty() { None } else { Some(seg) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_mirrors_php() {
        assert!(check("netease", "playlist", "123"));
        assert!(check("kugou", "song", "abc"));
        assert!(!check("spotify", "song", "1"));
        assert!(!check("netease", "mv", "1"));
        assert!(!check("netease", "song", ""));
    }

    #[test]
    fn parse_music_url_shapes() {
        assert_eq!(
            parse_music_url("https://music.163.com/#/playlist?id=12345"),
            ("netease".to_string(), "12345".to_string(), "playlist".to_string())
        );
        assert_eq!(
            parse_music_url("https://music.163.com/song?id=678"),
            ("netease".to_string(), "678".to_string(), "song".to_string())
        );
        assert_eq!(
            parse_music_url("https://y.qq.com/n/ryqq/playlist/abc123.html"),
            ("tencent".to_string(), "abc123".to_string(), "playlist".to_string())
        );
        assert_eq!(parse_music_url("https://example.com/x"), (String::new(), String::new(), String::new()));
        assert_eq!(parse_music_url(""), (String::new(), String::new(), String::new()));
    }
}
