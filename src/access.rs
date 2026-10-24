//! Access 内置插件的数据层：访问日志表、`contents.views` 浏览数、前端打点脚本。
//!
//! 对齐参考 `Access/sql/SQLite.sql` 与 `Plugin.php frontend()`：
//! 日志表为 `{prefix}access_log`；公开路由 `GET /access/log/track.gif`
//! 按 `u/cid/mid/rand` 记录后返回 1x1 GIF；前端写入模式（`writeType=1`）
//! 在页脚注入 `window.Access.track`；浏览数以 `contents.views` 为准，
//! cookie 去重键对齐 `extend_contents_views`。
//! 入库列对齐 `Access_Core::writeLogs`：UA 解析（browser/os/robot）、
//! 入口页（entrypoint，见 `resolve_entrypoint`）与 referer 双列。

use crate::access_ua;
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

/// 入口页 cookie 名（对齐 `Access_Core::getEntryPoint` 的 `__typecho_access_entrypoint`，
/// 调用方按惯例经 `auth::cookie_name(siteUrl, key)` 加前缀）。
pub const ENTRYPOINT_COOKIE: &str = "__typecho_access_entrypoint";

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

/// `access_log` 建表 DDL（SQLite/MySQL 方言分支，单测可直接断言字符串）。
pub fn create_table_sql(dialect: &Dialect, table: &str) -> String {
    match dialect {
        Dialect::Sqlite => format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
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
            "CREATE TABLE IF NOT EXISTS {table} (\
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
    }
}

/// `access_log` 辅助索引 DDL（MySQL 对 `IF NOT EXISTS` 支持不一，调用方忽略建失败）。
pub fn create_index_sql(dialect: &Dialect, prefix: &str, name: &str, cols: &str) -> String {
    let t = format!("{prefix}access_log");
    match dialect {
        Dialect::Sqlite => format!("CREATE INDEX IF NOT EXISTS \"{t}_{name}\" ON {t} ({cols})"),
        Dialect::Mysql => format!("CREATE INDEX {t}_{name} ON {t} ({cols})"),
    }
}

/// 激活时确保日志表存在（SQLite/MySQL 方言分支）。
pub async fn ensure_table(db: &Db) -> Result<(), sqlx::Error> {
    let t = db.table("access_log");
    sqlx::query(&create_table_sql(&db.dialect, &t)).execute(&db.pool).await?;
    // 索引建失败不阻断启用（MySQL 对 IF NOT EXISTS 支持不一）。
    for (name, cols) in [
        ("time", "time"),
        ("path", "path"),
        ("content_id", "content_id"),
        ("meta_id", "meta_id"),
    ] {
        let _ = sqlx::query(&create_index_sql(&db.dialect, &db.prefix, name, cols))
            .execute(&db.pool)
            .await;
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
/// 入库列对齐 `Access_Core::writeLogs`：UA 解析、entrypoint 与 referer 双列。
#[allow(clippy::too_many_arguments)]
pub async fn log_visit(
    db: &Db,
    ua: &str,
    url: &str,
    ip: &str,
    referer: &str,
    entrypoint: &str,
    now: i64,
    cid: Option<i64>,
    mid: Option<i64>,
) -> Result<(), sqlx::Error> {
    let (path, query) = split_url(url);
    let ip_num = ip_to_int(ip);
    let parsed = access_ua::parse(ua);
    let sql = format!(
        "INSERT INTO {} (ua, browser_id, browser_version, os_id, os_version, \
         url, path, query_string, ip, referer, referer_domain, entrypoint, entrypoint_domain, \
         time, content_id, meta_id, robot, robot_id, robot_version) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        db.table("access_log")
    );
    let mut q = sqlx::query(&sql)
        .bind(trunc(ua, 512))
        .bind(trunc(&parsed.browser_id, 32))
        .bind(trunc(&parsed.browser_version, 32))
        .bind(trunc(&parsed.os_id, 32))
        .bind(trunc(&parsed.os_version, 32))
        .bind(trunc(url, 255))
        .bind(trunc(&path, 255))
        .bind(trunc(&query, 255))
        .bind(ip_num)
        .bind(trunc(referer, 255))
        .bind(trunc(&domain_of(referer), 100))
        .bind(trunc(entrypoint, 255))
        .bind(trunc(&domain_of(entrypoint), 100))
        .bind(now);
    match cid {
        Some(id) => q = q.bind(id),
        None => q = q.bind(None::<i64>),
    }
    match mid {
        Some(id) => q = q.bind(id),
        None => q = q.bind(None::<i64>),
    }
    q = q
        .bind(i64::from(parsed.robot))
        .bind(trunc(&parsed.robot_id, 32))
        .bind(trunc(&parsed.robot_version, 32));
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
    host_of(url).unwrap_or_default()
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
}

/// 对齐 `Access_Core::getEntryPoint`：优先 Referer，空则读 entrypoint cookie；
/// 与站点同 host 视为站内（置空）；外链返回 `(entrypoint, true)` 提示调用方写 cookie。
///
/// `site_base` 为站点根（如 `http://test.local`），`cookie` 为已加前缀的
/// `__typecho_access_entrypoint` 现值。
pub fn resolve_entrypoint(site_base: &str, referer: &str, cookie: &str) -> (String, bool) {
    let mut entrypoint = if referer.is_empty() { cookie } else { referer };
    if !entrypoint.is_empty() && host_of(entrypoint) == host_of(site_base) {
        entrypoint = "";
    }
    let needs_set = !entrypoint.is_empty() && entrypoint != cookie;
    (entrypoint.to_string(), needs_set)
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

/// KeyCDN 归属查询基址（对齐 `Action::ip` 的 `tools.keycdn.com/geo.json`）。
pub const KEYCDN_BASE: &str = "https://tools.keycdn.com";

/// 请求 KeyCDN，成功返回 `"{country} {city}"`（trim 过）。
/// `base` 可注入（单测指向本地 mock），生产传 [`KEYCDN_BASE`]。
pub async fn keycdn_lookup(base: &str, ip: &str) -> Option<String> {
    let url = format!("{base}/geo.json?host={ip}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(&resp.text().await.ok()?).ok()?;
    if body.get("status")?.as_str()? != "success" {
        return None;
    }
    let geo = body.pointer("/data/geo")?;
    let country = geo.get("country_name")?.as_str().unwrap_or("");
    let city = geo.get("city")?.as_str().filter(|s| !s.is_empty());
    let label = match city {
        Some(city) => format!("{country} {city}"),
        None => country.to_string(),
    };
    let label = label.trim().to_string();
    if label.is_empty() { None } else { Some(label) }
}

/// 对齐 `Action::ip`：本地可判定（回环/内网等）直接回 `code 0`；
/// 公网走 KeyCDN，成功 `code 0`，都失败 `code 100`（与 PHP 相近文案）。
pub async fn geo_for_ip_with(ip_raw: &str, keycdn_base: &str) -> (i64, String) {
    let Ok(addr) = ip_raw.trim().parse::<std::net::Ipv4Addr>() else {
        return (100, "解析ip失败".to_string());
    };
    let label = if addr.is_loopback() {
        "回环地址"
    } else if addr.is_private() {
        "内网地址"
    } else if addr.is_link_local() {
        "链路本地地址"
    } else if addr.is_multicast() {
        "组播地址"
    } else if addr.is_broadcast() || addr.is_unspecified() || addr.is_documentation() {
        "保留地址"
    } else {
        let ip = addr.to_string();
        return match keycdn_lookup(keycdn_base, &ip).await {
            Some(geo) => (0, geo),
            None => (
                100,
                "很抱歉，ipip.net查询无结果，同时你的服务器无法连接fallback接口(tools.keycdn.com)"
                    .to_string(),
            ),
        };
    };
    (0, format!("{addr} {label}"))
}

/// 生产入口：KeyCDN 基址固定为 [`KEYCDN_BASE`]。
pub async fn geo_for_ip(ip_raw: &str) -> (i64, String) {
    geo_for_ip_with(ip_raw, KEYCDN_BASE).await
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

/// UV 表达式：去重 `(ip, ua)`。MySQL 原生支持多列 `DISTINCT`，SQLite 用分隔拼接等价。
fn uv_expr(dialect: &Dialect) -> &'static str {
    match dialect {
        Dialect::Mysql => "COUNT(DISTINCT ip, ua)",
        Dialect::Sqlite => "COUNT(DISTINCT ip || CHAR(7) || ua)",
    }
}

/// 今日按小时分组的桶表达式（`time` 落在 `[ts, te]` 内时值为 0..=23）。
fn hour_expr(dialect: &Dialect) -> String {
    match dialect {
        // SQLite 整数除法截断即取整；MySQL 用 FLOOR。
        Dialect::Sqlite => "CAST((time - ?) / 3600 AS INTEGER)".to_string(),
        Dialect::Mysql => "FLOOR((time - ?) / 3600)".to_string(),
    }
}

pub async fn overview(db: &Db, now: i64, tz_secs: i64) -> Overview {
    let (ts, te) = day_bounds(now, tz_secs, 0);
    let (ys, _) = day_bounds(now, tz_secs, 1);
    let t = db.table("access_log");
    let uv = uv_expr(&db.dialect);
    let mut ov = Overview {
        pv_today: 0,
        uv_today: 0,
        ip_today: 0,
        pv_yesterday: 0,
        uv_yesterday: 0,
        ip_yesterday: 0,
        pv_all: 0,
        uv_all: 0,
        ip_all: 0,
        hours: (0..24)
            .map(|hour| HourStat { hour, pv: 0, uv: 0, ip: 0 })
            .collect(),
    };
    // Q1：今日 24 小时分布，一次 GROUP BY 扫完。
    let hour_sql = format!(
        "SELECT {} AS h, COUNT(*) AS pv, COUNT(DISTINCT ip) AS ipc, {uv} AS uvc \
         FROM {t} WHERE time >= ? AND time <= ? GROUP BY h",
        hour_expr(&db.dialect)
    );
    if let Ok(rows) = sqlx::query(&hour_sql).bind(ts).bind(ts).bind(te).fetch_all(&db.pool).await {
        for row in &rows {
            let h = col_i64(row, "h").unwrap_or(-1);
            if (0..24).contains(&h) {
                let slot = &mut ov.hours[h as usize];
                slot.pv = col_i64(row, "pv").unwrap_or(0);
                slot.ip = col_i64(row, "ipc").unwrap_or(0);
                slot.uv = col_i64(row, "uvc").unwrap_or(0);
            }
        }
    }
    // Q2：今日/昨日总量，一条按天分组。
    let day_sql = format!(
        "SELECT (CASE WHEN time >= ? THEN 0 ELSE 1 END) AS d, COUNT(*) AS pv, \
         COUNT(DISTINCT ip) AS ipc, {uv} AS uvc \
         FROM {t} WHERE time >= ? AND time <= ? GROUP BY d"
    );
    if let Ok(rows) = sqlx::query(&day_sql).bind(ts).bind(ys).bind(te).fetch_all(&db.pool).await {
        for row in &rows {
            match col_i64(row, "d").unwrap_or(-1) {
                0 => {
                    ov.pv_today = col_i64(row, "pv").unwrap_or(0);
                    ov.ip_today = col_i64(row, "ipc").unwrap_or(0);
                    ov.uv_today = col_i64(row, "uvc").unwrap_or(0);
                }
                1 => {
                    ov.pv_yesterday = col_i64(row, "pv").unwrap_or(0);
                    ov.ip_yesterday = col_i64(row, "ipc").unwrap_or(0);
                    ov.uv_yesterday = col_i64(row, "uvc").unwrap_or(0);
                }
                _ => {}
            }
        }
    }
    // Q3：全站总量。
    let all_sql = format!("SELECT COUNT(*) AS pv, COUNT(DISTINCT ip) AS ipc, {uv} AS uvc FROM {t}");
    if let Ok(Some(row)) = sqlx::query(&all_sql).fetch_optional(&db.pool).await {
        ov.pv_all = col_i64(&row, "pv").unwrap_or(0);
        ov.ip_all = col_i64(&row, "ipc").unwrap_or(0);
        ov.uv_all = col_i64(&row, "uvc").unwrap_or(0);
    }
    ov
}

/// 来源域名 / 来源页 Top，对齐 PHP `parseReferer`：按 **entrypoint** 聚合，不是 referer。
pub async fn top_referers(db: &Db, limit: i64) -> (Vec<(String, i64)>, Vec<(String, i64)>) {
    let t = db.table("access_log");
    let domains = sqlx::query(&format!(
        "SELECT entrypoint_domain AS value, COUNT(*) AS n FROM {t} \
         WHERE entrypoint_domain <> '' GROUP BY entrypoint_domain ORDER BY n DESC LIMIT ?"
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
        "SELECT entrypoint AS value, COUNT(*) AS n FROM {t} \
         WHERE entrypoint <> '' GROUP BY entrypoint ORDER BY n DESC LIMIT ?"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_covers_both_dialects() {
        let lite = create_table_sql(&Dialect::Sqlite, "typecho_access_log");
        assert!(lite.contains("AUTOINCREMENT"));
        assert!(lite.contains("entrypoint_domain"));
        assert!(lite.contains("robot_id"));
        let mysql = create_table_sql(&Dialect::Mysql, "typecho_access_log");
        assert!(mysql.contains("AUTO_INCREMENT"));
        assert!(mysql.contains("TINYINT(1)"));
        assert!(mysql.contains("entrypoint VARCHAR(255)"));
        let lite_idx = create_index_sql(&Dialect::Sqlite, "typecho_", "time", "time");
        assert!(lite_idx.contains("IF NOT EXISTS"));
        let mysql_idx = create_index_sql(&Dialect::Mysql, "typecho_", "time", "time");
        assert!(!mysql_idx.contains("IF NOT EXISTS"));
        assert!(mysql_idx.contains("typecho_access_log_time"));
    }

    #[test]
    fn entrypoint_resolution() {
        // 外链：记 entrypoint 并提示写 cookie。
        let (ep, set) = resolve_entrypoint("http://test.local", "https://ext.example/a", "");
        assert_eq!(ep, "https://ext.example/a");
        assert!(set);
        // 同站：置空且不写。
        let (ep, set) = resolve_entrypoint("http://test.local", "http://test.local/archives/1/", "https://ext.example/a");
        assert_eq!(ep, "");
        assert!(!set);
        // 无 Referer：沿用 cookie，不重复写。
        let (ep, set) = resolve_entrypoint("http://test.local", "", "https://ext.example/a");
        assert_eq!(ep, "https://ext.example/a");
        assert!(!set);
        // 都空：空且不写。
        let (ep, set) = resolve_entrypoint("http://test.local", "", "");
        assert_eq!(ep, "");
        assert!(!set);
    }

    async fn mock_keycdn(status: &str, body: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let resp = format!(
                    "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn keycdn_success_and_failure_shapes() {
        let base = mock_keycdn(
            "HTTP/1.1 200 OK",
            r#"{"status":"success","data":{"geo":{"country_name":"United States","city":"Mountain View"}}}"#,
        )
        .await;
        assert_eq!(
            keycdn_lookup(&base, "8.8.8.8").await.as_deref(),
            Some("United States Mountain View")
        );
        let base = mock_keycdn("HTTP/1.1 200 OK", r#"{"status":"error"}"#).await;
        assert_eq!(keycdn_lookup(&base, "8.8.8.8").await, None);
        let base = mock_keycdn("HTTP/1.1 500 Boom", "x").await;
        assert_eq!(keycdn_lookup(&base, "8.8.8.8").await, None);
    }

    #[tokio::test]
    async fn geo_local_and_fallback() {
        let (code, data) = geo_for_ip_with("127.0.0.1", "http://127.0.0.1:9").await;
        assert_eq!((code, data.as_str()), (0, "127.0.0.1 回环地址"));
        let (code, data) = geo_for_ip_with("192.168.1.2", "http://127.0.0.1:9").await;
        assert_eq!(code, 0);
        assert!(data.contains("内网地址"), "{data}");
        let (code, data) = geo_for_ip_with("bogus", "http://127.0.0.1:9").await;
        assert_eq!((code, data.as_str()), (100, "解析ip失败"));
        // 公网 + 不可达基址 → code 100 的 fallback 文案。
        let (code, data) = geo_for_ip_with("8.8.8.8", "http://127.0.0.1:9").await;
        assert_eq!(code, 100);
        assert!(data.contains("tools.keycdn.com"), "{data}");
    }

    #[tokio::test]
    async fn log_visit_writes_ua_and_entrypoint_columns() {
        let db = Db::connect("sqlite::memory:", "t_").await.unwrap();
        ensure_table(&db).await.unwrap();
        log_visit(
            &db,
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "/feed/",
            "203.0.113.7",
            "https://ext.example/a",
            "https://ext.example/a",
            1_700_000_100,
            None,
            None,
        )
        .await
        .unwrap();
        log_visit(
            &db,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/126.0.0.0 Safari/537.36",
            "/",
            "198.51.100.9",
            "",
            "",
            1_700_000_100,
            None,
            Some(0),
        )
        .await
        .unwrap();
        let rows = sqlx::query(
            r#"SELECT robot, robot_id, browser_id, os_id, entrypoint, entrypoint_domain FROM "t_access_log" ORDER BY id"#,
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(col_i64(&rows[0], "robot").unwrap(), 1);
        assert!(!col_str(&rows[0], "robot_id").is_empty());
        assert_eq!(col_str(&rows[0], "entrypoint"), "https://ext.example/a");
        assert_eq!(col_str(&rows[0], "entrypoint_domain"), "ext.example");
        assert_eq!(col_str(&rows[1], "browser_id"), "Chrome");
        assert_eq!(col_str(&rows[1], "os_id"), "Windows");
        assert_eq!(col_i64(&rows[1], "robot").unwrap(), 0);
    }

    #[tokio::test]
    async fn overview_uses_grouped_queries() {
        let db = Db::connect("sqlite::memory:", "t_").await.unwrap();
        ensure_table(&db).await.unwrap();
        let now = 1_700_000_000_i64;
        let (ts, _) = day_bounds(now, 0, 0);
        let (ys, _) = day_bounds(now, 0, 1);
        for (ua, ip, at) in [
            ("ua-a", "10.0.0.1", ts + 100),
            ("ua-b", "10.0.0.1", ts + 200),
            ("ua-c", "10.0.0.2", ts + 5 * 3600),
            ("ua-d", "10.0.0.3", ys + 100),
        ] {
            log_visit(&db, ua, "/", ip, "", "", at, None, None).await.unwrap();
        }
        let ov = overview(&db, now, 0).await;
        assert_eq!((ov.pv_today, ov.uv_today, ov.ip_today), (3, 3, 2));
        assert_eq!((ov.pv_yesterday, ov.uv_yesterday, ov.ip_yesterday), (1, 1, 1));
        assert_eq!((ov.pv_all, ov.uv_all, ov.ip_all), (4, 4, 3));
        assert_eq!(ov.hours.len(), 24);
        assert_eq!(ov.hours[0].pv, 2);
        assert_eq!(ov.hours[5].pv, 1);
        assert_eq!(ov.hours[23].pv, 0);
        // 来源统计走 entrypoint（PHP parseReferer 口径）。
        let (domains, urls) = top_referers(&db, 10).await;
        assert!(domains.is_empty() && urls.is_empty());
    }
}
