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
        "INSERT INTO {} (ua, url, path, query_string, ip, referer, referer_domain, time, content_id, meta_id, robot) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
        db.table("access_log")
    );
    let mut q = sqlx::query(&sql)
        .bind(trunc(ua, 512))
        .bind(trunc(url, 255))
        .bind(trunc(&path, 255))
        .bind(trunc(&query, 255))
        .bind(ip_num)
        .bind(trunc(referer, 255))
        .bind(trunc(&domain_of(referer), 100))
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

/// 取 URL 的 host（无则空串）。Rust 新行用它填 `referer_domain`，旧 PHP 行本来就有。
fn domain_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
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
/// `cid`/`mid` 按 `Access_Core::parseArchive` 语义由调用方传入：文章/页面传 cid，
/// 分类/标签传 mid，首页传 mid=0，其余留空（PHP 里 null 插值即空字符串）。
pub fn frontend_snippet(
    site: &Site,
    write_type: &str,
    cid: Option<i64>,
    mid: Option<i64>,
) -> String {
    if write_type != "1" {
        return String::new();
    }
    let gif = links::with_index(site, "/access/log/track.gif");
    let cid = cid.map(|v| v.to_string()).unwrap_or_default();
    let mid = mid.map(|v| v.to_string()).unwrap_or_default();
    format!(
        "<script type=\"text/javascript\">window.Access={{track:function(){{var i=new Image();i.src='{gif}?u='+encodeURIComponent(location.pathname+location.search+location.hash)+'&cid={cid}&mid={mid}&rand='+new Date().getTime()}}}};window.Access.track();</script>"
    )
}

/// 日志列表筛选，对齐控制台 `filter`：all / ip / post（cid）/ path。
#[derive(Debug, Clone, Default)]
pub enum LogFilter {
    #[default]
    All,
    Ip(i64),
    Content(i64),
    Path(String),
}

/// 日志类型筛选，对齐控制台 `type`：1 仅人类 / 2 仅爬虫 / 3 所有。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Human,
    Robot,
    All,
}

impl LogKind {
    pub fn from_param(raw: &str) -> Self {
        match raw {
            "2" => LogKind::Robot,
            "3" => LogKind::All,
            _ => LogKind::Human,
        }
    }
}

fn filtered_where(filter: &LogFilter, kind: LogKind) -> (String, Vec<i64>, Vec<String>) {
    let mut conds = Vec::new();
    let mut ints = Vec::new();
    let mut strs = Vec::new();
    match kind {
        LogKind::Human => conds.push("robot = 0".to_string()),
        LogKind::Robot => conds.push("robot = 1".to_string()),
        LogKind::All => {}
    }
    match filter {
        LogFilter::All => {}
        LogFilter::Ip(ip) => {
            conds.push("ip = ?".to_string());
            ints.push(*ip);
        }
        LogFilter::Content(cid) => {
            conds.push("content_id = ?".to_string());
            ints.push(*cid);
        }
        LogFilter::Path(path) => {
            conds.push("path = ?".to_string());
            strs.push(path.clone());
        }
    }
    let sql = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };
    (sql, ints, strs)
}

fn bind_filtered<'a>(
    q: sqlx::query::Query<'a, sqlx::Any, sqlx::any::AnyArguments<'a>>,
    ints: &'a [i64],
    strs: &'a [String],
) -> sqlx::query::Query<'a, sqlx::Any, sqlx::any::AnyArguments<'a>> {
    let mut q = q;
    for n in ints {
        q = q.bind(*n);
    }
    for s in strs {
        q = q.bind(s.as_str());
    }
    q
}

pub async fn recent_logs_filtered(
    db: &Db,
    filter: &LogFilter,
    kind: LogKind,
    limit: i64,
    offset: i64,
) -> Result<Vec<AccessLog>, sqlx::Error> {
    let (cond, ints, strs) = filtered_where(filter, kind);
    let sql = format!(
        "SELECT id, url, path, ip, referer, time, content_id, meta_id FROM {} \
         {cond} ORDER BY time DESC, id DESC LIMIT ? OFFSET ?",
        db.table("access_log")
    );
    let q = bind_filtered(sqlx::query(&sql), &ints, &strs).bind(limit).bind(offset);
    let rows = q.fetch_all(&db.pool).await?;
    Ok(rows.iter().map(map_log).collect())
}

pub async fn log_count_filtered(
    db: &Db,
    filter: &LogFilter,
    kind: LogKind,
) -> Result<i64, sqlx::Error> {
    let (cond, ints, strs) = filtered_where(filter, kind);
    let sql = format!("SELECT COUNT(*) AS n FROM {} {cond}", db.table("access_log"));
    let q = bind_filtered(sqlx::query(&sql), &ints, &strs);
    let row = q.fetch_optional(&db.pool).await?;
    Ok(row
        .as_ref()
        .map(|r| col_i64(r, "n").unwrap_or(0))
        .unwrap_or(0))
}

/// 批量删除（对齐 `Access_Core::deleteLogs`）。返回删掉的行数。
pub async fn delete_logs(db: &Db, ids: &[i64]) -> Result<u64, sqlx::Error> {
    let ids: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!("DELETE FROM {} WHERE id IN ({marks})", db.table("access_log"));
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    Ok(q.execute(&db.pool).await?.rows_affected())
}

fn map_log(row: &sqlx::any::AnyRow) -> AccessLog {
    AccessLog {
        id: col_i64(row, "id").unwrap_or(0),
        url: col_str(row, "url"),
        path: col_str(row, "path"),
        ip: int_to_ip(col_i64(row, "ip").unwrap_or(0)),
        referer: col_str(row, "referer"),
        time: col_i64(row, "time").unwrap_or(0),
        content_id: row_content_id(row, "content_id"),
        meta_id: row_content_id(row, "meta_id"),
    }
}

/// 访问概览：今日/昨日/总计的 PV、UV（ip+ua 去重）、IP（去重），及今日 24 小时分布。
/// 天边界按站点时区算（PHP 用服务器本地日期）。
#[derive(Debug, Clone)]
pub struct HourStat {
    pub hour: u32,
    pub pv: i64,
    pub uv: i64,
    pub ip: i64,
}

#[derive(Debug, Clone)]
pub struct Overview {
    pub pv_today: i64,
    pub uv_today: i64,
    pub ip_today: i64,
    pub pv_yesterday: i64,
    pub uv_yesterday: i64,
    pub ip_yesterday: i64,
    pub pv_all: i64,
    pub uv_all: i64,
    pub ip_all: i64,
    pub hours: Vec<HourStat>,
}

/// 站点时区下某天的 `[start, end]`（闭区间，与 PHP `00:00:00..23:59:59` 同向）。
fn day_bounds(now: i64, tz_secs: i64, days_ago: i64) -> (i64, i64) {
    let day = (now + tz_secs).div_euclid(86400) - days_ago;
    let start = day * 86400 - tz_secs;
    (start, start + 86400 - 1)
}

async fn count_between(db: &Db, start: i64, end: i64) -> (i64, i64, i64) {
    let t = db.table("access_log");
    let pv: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {t} WHERE time >= ? AND time <= ?"
    ))
    .bind(start)
    .bind(end)
    .fetch_one(&db.pool)
    .await
    .unwrap_or(0);
    // DISTINCT 子查询两种方言通用，避免拼接函数差异。
    let uv: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM (SELECT DISTINCT ip, ua FROM {t} WHERE time >= ? AND time <= ?)"
    ))
    .bind(start)
    .bind(end)
    .fetch_one(&db.pool)
    .await
    .unwrap_or(0);
    let ip: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM (SELECT DISTINCT ip FROM {t} WHERE time >= ? AND time <= ?)"
    ))
    .bind(start)
    .bind(end)
    .fetch_one(&db.pool)
    .await
    .unwrap_or(0);
    (pv, uv, ip)
}

pub async fn overview(db: &Db, now: i64, tz_secs: i64) -> Overview {
    let (ts, te) = day_bounds(now, tz_secs, 0);
    let (ys, ye) = day_bounds(now, tz_secs, 1);
    let (pv_today, uv_today, ip_today) = count_between(db, ts, te).await;
    let (pv_yesterday, uv_yesterday, ip_yesterday) = count_between(db, ys, ye).await;
    let t = db.table("access_log");
    let pv_all: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {t}"))
        .fetch_one(&db.pool)
        .await
        .unwrap_or(0);
    let uv_all: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM (SELECT DISTINCT ip, ua FROM {t})"))
            .fetch_one(&db.pool)
            .await
            .unwrap_or(0);
    let ip_all: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM (SELECT DISTINCT ip FROM {t})"))
            .fetch_one(&db.pool)
            .await
            .unwrap_or(0);
    let mut hours = Vec::with_capacity(24);
    for hour in 0..24 {
        let (hs, he) = (ts + hour as i64 * 3600, ts + hour as i64 * 3600 + 3599);
        let (pv, uv, ip) = count_between(db, hs, he).await;
        hours.push(HourStat {
            hour: hour as u32,
            pv,
            uv,
            ip,
        });
    }
    Overview {
        pv_today,
        uv_today,
        ip_today,
        pv_yesterday,
        uv_yesterday,
        ip_yesterday,
        pv_all,
        uv_all,
        ip_all,
        hours,
    }
}

/// 来源域名 / 来源页 Top（按 `referer_domain` / `referer` 聚合，两边引擎都会写这两列）。
pub async fn top_referers(db: &Db, limit: i64) -> (Vec<(String, i64)>, Vec<(String, i64)>) {
    let t = db.table("access_log");
    let domains = sqlx::query(&format!(
        "SELECT referer_domain AS value, COUNT(*) AS n FROM {t} \
         WHERE referer_domain <> '' GROUP BY referer_domain ORDER BY n DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(&db.pool)
    .await
    .map(|rows| {
        rows.iter()
            .map(|r| (col_str(r, "value"), col_i64(r, "n").unwrap_or(0)))
            .collect()
    })
    .unwrap_or_default();
    let urls = sqlx::query(&format!(
        "SELECT referer AS value, COUNT(*) AS n FROM {t} \
         WHERE referer <> '' GROUP BY referer ORDER BY n DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(&db.pool)
    .await
    .map(|rows| {
        rows.iter()
            .map(|r| (col_str(r, "value"), col_i64(r, "n").unwrap_or(0)))
            .collect()
    })
    .unwrap_or_default();
    (domains, urls)
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
    Ok(rows.iter().map(map_log).collect())
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
