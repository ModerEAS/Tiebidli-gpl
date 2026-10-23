//! Access 内置插件的数据层：访问日志表、`contents.views` 浏览数、前端打点脚本。
//!
//! 对齐参考 `Access/sql/SQLite.sql` 与 `Plugin.php frontend()`：
//! 日志表为 `{prefix}access_log`；公开路由 `GET /access/log/track.gif`
//! 按 `u/cid/mid/rand` 记录后返回 1x1 GIF；前端写入模式（`writeType=1`）
//! 在页脚注入 `window.Access.track`；浏览数以 `contents.views` 为准，
//! cookie 去重键对齐 `extend_contents_views`。

use crate::db::{col_i64, col_str, Db, Dialect};
use crate::links;
use crate::model::Site;

/// 1x1 透明 GIF，与参考 `Action::writeLogs` 内嵌像素一致。
pub const TRACK_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x0A, 0x00, 0x01, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x4C, 0x01, 0x00, 0x3B,
];

/// 浏览去重 cookie 名（需调用方按站点根加前缀）。
pub const VIEWS_COOKIE: &str = "extend_contents_views";

#[derive(Debug, Clone)]
pub struct AccessLog {
    pub id: i64,
    pub url: String,
    pub path: String,
    pub ip: String,
    pub referer: String,
    pub time: i64,
    pub content_id: Option<i64>,
    pub meta_id: Option<i64>,
}

/// 激活时确保日志表存在（SQLite/MySQL 方言分支）。
pub async fn ensure_table(db: &Db) -> Result<(), sqlx::Error> {
    let t = db.table("access_log");
    let create = match db.dialect {
        Dialect::Sqlite => format!(
            "CREATE TABLE IF NOT EXISTS {t} (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             ua TEXT DEFAULT '', browser_id TEXT DEFAULT '', browser_version TEXT DEFAULT '', \
             os_id TEXT DEFAULT '', os_version TEXT DEFAULT '', url TEXT DEFAULT '', \
             path TEXT DEFAULT '', query_string TEXT DEFAULT '', ip INTEGER DEFAULT 0, \
             entrypoint TEXT DEFAULT '', entrypoint_domain TEXT DEFAULT '', \
             referer TEXT DEFAULT '', referer_domain TEXT DEFAULT '', time INTEGER DEFAULT 0, \
             content_id INTEGER DEFAULT NULL, meta_id INTEGER DEFAULT NULL, \
             robot INTEGER DEFAULT 0, robot_id TEXT DEFAULT '', robot_version TEXT DEFAULT '')"
        ),
        Dialect::Mysql => format!(
            "CREATE TABLE IF NOT EXISTS {t} (\
             id INT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             ua VARCHAR(512) DEFAULT '', browser_id VARCHAR(32) DEFAULT '', \
             browser_version VARCHAR(32) DEFAULT '', os_id VARCHAR(32) DEFAULT '', \
             os_version VARCHAR(32) DEFAULT '', url VARCHAR(255) DEFAULT '', \
             path VARCHAR(255) DEFAULT '', query_string VARCHAR(255) DEFAULT '', \
             ip BIGINT DEFAULT 0, entrypoint VARCHAR(255) DEFAULT '', \
             entrypoint_domain VARCHAR(100) DEFAULT '', referer VARCHAR(255) DEFAULT '', \
             referer_domain VARCHAR(100) DEFAULT '', time INT DEFAULT 0, \
             content_id INT DEFAULT NULL, meta_id INT DEFAULT NULL, \
             robot TINYINT(1) DEFAULT 0, robot_id VARCHAR(32) DEFAULT '', \
             robot_version VARCHAR(32) DEFAULT '')"
        ),
    };
    sqlx::query(&create).execute(&db.pool).await?;
    // 索引建失败不阻断启用（MySQL 对 IF NOT EXISTS 支持不一）。
    for (name, cols) in [
        ("time", "time"),
        ("path", "path"),
        ("content_id", "content_id"),
        ("meta_id", "meta_id"),
    ] {
        let idx = match db.dialect {
            Dialect::Sqlite => format!(
                "CREATE INDEX IF NOT EXISTS \"{p}access_log_{name}\" ON {t} ({cols})",
                p = db.prefix,
            ),
            Dialect::Mysql => format!("CREATE INDEX {p}access_log_{name} ON {t} ({cols})", p = db.prefix),
        };
        let _ = sqlx::query(&idx).execute(&db.pool).await;
    }
    Ok(())
}

/// 确保 `contents.views` 列存在，不存在则 `ALTER` 补上。
pub async fn ensure_views_column(db: &Db) -> Result<(), sqlx::Error> {
    let probe = format!("SELECT views FROM {} LIMIT 1", db.table("contents"));
    if sqlx::query(&probe).fetch_optional(&db.pool).await.is_ok() {
        return Ok(());
    }
    let alter = match db.dialect {
        Dialect::Sqlite => format!(
            "ALTER TABLE {} ADD COLUMN views INTEGER DEFAULT 0",
            db.table("contents")
        ),
        Dialect::Mysql => format!(
            "ALTER TABLE {} ADD COLUMN views INT DEFAULT 0",
            db.table("contents")
        ),
    };
    // 列已存在时的竞态直接忽略，由调用方继续。
    let _ = sqlx::query(&alter).execute(&db.pool).await;
    Ok(())
}

pub async fn table_exists(db: &Db) -> bool {
    let probe = format!("SELECT id FROM {} LIMIT 1", db.table("access_log"));
    sqlx::query(&probe).fetch_optional(&db.pool).await.is_ok()
}

/// 写入一条访问日志。`u` 为前端传来的受访地址（含 path+query+hash）。
#[allow(clippy::too_many_arguments)]
pub async fn log_visit(
    db: &Db,
    ua: &str,
    url: &str,
    ip: &str,
    referer: &str,
    now: i64,
    cid: Option<i64>,
    mid: Option<i64>,
) -> Result<(), sqlx::Error> {
    let (path, query) = split_url(url);
    let ip_num = ip_to_int(ip);
    let sql = format!(
        "INSERT INTO {} (ua, url, path, query_string, ip, referer, time, content_id, meta_id, robot) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
        db.table("access_log")
    );
    let mut q = sqlx::query(&sql)
        .bind(trunc(ua, 512))
        .bind(trunc(url, 255))
        .bind(trunc(&path, 255))
        .bind(trunc(&query, 255))
        .bind(ip_num)
        .bind(trunc(referer, 255))
        .bind(now);
    match cid {
        Some(id) => q = q.bind(id),
        None => q = q.bind(None::<i64>),
    }
    match mid {
        Some(id) => q = q.bind(id),
        None => q = q.bind(None::<i64>),
    }
    q.execute(&db.pool).await?;
    Ok(())
}

fn split_url(url: &str) -> (String, String) {
    let no_hash = url.split('#').next().unwrap_or(url);
    match no_hash.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (no_hash.to_string(), String::new()),
    }
}

fn ip_to_int(ip: &str) -> i64 {
    let ip = ip.trim();
    if let Ok(v4) = ip.parse::<std::net::Ipv4Addr>() {
        return u32::from(v4) as i64;
    }
    // IPv6/非法统一记 0，与旧版 `ip2long` 失败语义一致。
    0
}

fn trunc(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 读取 `contents.views`，列缺失时返回 0（不抛错）。
pub async fn get_views(db: &Db, cid: i64) -> i64 {
    let sql = format!("SELECT views FROM {} WHERE cid = ?", db.table("contents"));
    let Ok(Some(row)) = sqlx::query(&sql).bind(cid).fetch_optional(&db.pool).await else {
        return 0;
    };
    col_i64(&row, "views").unwrap_or(0).max(0)
}

/// `views + 1` 并返回新值；列缺失时先补列再写。
pub async fn increment_views(db: &Db, cid: i64) -> Result<i64, sqlx::Error> {
    let _ = ensure_views_column(db).await;
    let sql = format!(
        "UPDATE {} SET views = COALESCE(views, 0) + 1 WHERE cid = ?",
        db.table("contents")
    );
    sqlx::query(&sql).bind(cid).execute(&db.pool).await?;
    Ok(get_views(db, cid).await)
}

/// 去重 cookie 是否已看过该内容。
pub fn already_viewed(cookie: &str, cid: i64) -> bool {
    let needle = cid.to_string();
    cookie
        .split(',')
        .any(|part| part.trim() == needle)
}

/// 看过后把 cid 追加进 cookie（调用方负责 Set-Cookie）。
pub fn mark_viewed(cookie: &str, cid: i64) -> String {
    let mut ids: Vec<String> = cookie
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    // cookie 无限增长会超头，保留最近 200 个即可。
    if !ids.iter().any(|id| id == &cid.to_string()) {
        ids.push(cid.to_string());
    }
    if ids.len() > 200 {
        ids = ids.split_off(ids.len() - 200);
    }
    ids.join(",")
}

/// 前端写入模式（`writeType=1`）的页脚打点脚本，对齐参考 `frontend()`。
pub fn frontend_snippet(site: &Site, write_type: &str) -> String {
    if write_type != "1" {
        return String::new();
    }
    let gif = links::with_index(site, "/access/log/track.gif");
    format!(
        "<script type=\"text/javascript\">window.Access={{track:function(){{var i=new Image();i.src='{gif}?u='+encodeURIComponent(location.pathname+location.search+location.hash)+'&rand='+new Date().getTime()}}}};window.Access.track();</script>"
    )
}

pub async fn recent_logs(db: &Db, limit: i64, offset: i64) -> Result<Vec<AccessLog>, sqlx::Error> {
    let sql = format!(
        "SELECT id, url, path, ip, referer, time, content_id, meta_id FROM {} \
         ORDER BY time DESC, id DESC LIMIT ? OFFSET ?",
        db.table("access_log")
    );
    let rows = sqlx::query(&sql)
        .bind(limit)
        .bind(offset)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| AccessLog {
            id: col_i64(row, "id").unwrap_or(0),
            url: col_str(row, "url"),
            path: col_str(row, "path"),
            ip: int_to_ip(col_i64(row, "ip").unwrap_or(0)),
            referer: col_str(row, "referer"),
            time: col_i64(row, "time").unwrap_or(0),
            content_id: row_content_id(row, "content_id"),
            meta_id: row_content_id(row, "meta_id"),
        })
        .collect())
}

pub async fn log_count(db: &Db) -> Result<i64, sqlx::Error> {
    let sql = format!("SELECT COUNT(*) AS n FROM {}", db.table("access_log"));
    let row = sqlx::query(&sql).fetch_optional(&db.pool).await?;
    Ok(row
        .as_ref()
        .map(|r| col_i64(r, "n").unwrap_or(0))
        .unwrap_or(0))
}

/// 每个 `content_id` 的 PV（控制台按文章筛选的数据源）。
pub async fn pv_by_content(db: &Db) -> Result<Vec<(i64, String, i64)>, sqlx::Error> {
    let sql = format!(
        "SELECT l.content_id AS cid, COUNT(*) AS n, COALESCE(c.title, '') AS title \
         FROM {} l LEFT JOIN {} c ON l.content_id = c.cid \
         WHERE l.content_id IS NOT NULL GROUP BY l.content_id ORDER BY n DESC LIMIT 100",
        db.table("access_log"),
        db.table("contents"),
    );
    let rows = sqlx::query(&sql).fetch_all(&db.pool).await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                col_i64(row, "cid").unwrap_or(0),
                col_str(row, "title"),
                col_i64(row, "n").unwrap_or(0),
            )
        })
        .collect())
}

fn row_content_id(row: &sqlx::any::AnyRow, name: &str) -> Option<i64> {
    use sqlx::Row;
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(name) {
        return Some(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(name) {
        return Some(i64::from(v));
    }
    None
}

fn int_to_ip(n: i64) -> String {
    if n <= 0 {
        return "0.0.0.0".into();
    }
    std::net::Ipv4Addr::from(n as u32).to_string()
}
