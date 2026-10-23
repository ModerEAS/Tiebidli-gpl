//! Typecho 1.2 备份缓冲。
//!
//! 文件以 `%TYPECHO_BACKUP_0001%` 开头和结尾。中间每条记录是
//! `pack('vvV', type, header_len, body_len)`、JSON 字段长度、字段原文、以及这三段的 md5（32 位十六进制）。
//! 类型：contents=1，comments=2，metas=3，relationships=4，users=5，fields=6。
//! 不包含 options，因此设置（含外观）不会进出备份。
//!
//! `FILE` 版本按 6 字节头读取，正文长度由 JSON 里的非空字段相加。未知类型号是原版插件数据，这里跳过，核心表照常恢复。
//! 解析完整份文件之后才写入；解析失败时不改现有数据。
//!
//! 导出默认只下载。勾选「同时在服务器留一份」后，才在 `BACKUP_DIR` 写同名 `.dat`，
//! 供「从服务器」列表、下载和删除。下载和删除走 `/index.php/action/backup`，并校验 `_`。

use crate::auth;
use crate::config::Config;
use crate::db::Db;
use crate::model::Site;
use crate::text::{self, html_escape};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const HEADER_MARK: &str = "%TYPECHO_BACKUP_XXXX%";
const VERSION: &str = "0001";

const CONTENTS: &[&str] = &[
    "cid",
    "title",
    "slug",
    "created",
    "modified",
    "text",
    "order",
    "authorId",
    "template",
    "type",
    "status",
    "password",
    "commentsNum",
    "allowComment",
    "allowPing",
    "allowFeed",
    "parent",
];
const COMMENTS: &[&str] = &[
    "coid",
    "cid",
    "created",
    "author",
    "authorId",
    "ownerId",
    "mail",
    "url",
    "ip",
    "agent",
    "text",
    "type",
    "status",
    "parent",
];
const METAS: &[&str] = &[
    "mid",
    "name",
    "slug",
    "type",
    "description",
    "count",
    "order",
    "parent",
];
const RELATIONSHIPS: &[&str] = &["cid", "mid"];
const USERS: &[&str] = &[
    "uid",
    "name",
    "password",
    "mail",
    "url",
    "screenName",
    "created",
    "activated",
    "logged",
    "group",
    "authCode",
];
const FIELDS: &[&str] = &[
    "cid",
    "name",
    "type",
    "str_value",
    "int_value",
    "float_value",
];

#[derive(Debug)]
pub enum BackupFail {
    Format,
    Read,
    Missing,
    Empty,
    Upload,
    Data,
    Sql(String),
}

impl BackupFail {
    pub fn message(&self) -> String {
        match self {
            BackupFail::Format => "备份文件格式错误".into(),
            BackupFail::Read => "无法读取备份文件".into(),
            BackupFail::Missing => "备份文件不存在".into(),
            BackupFail::Empty => "没有选择任何备份文件".into(),
            BackupFail::Upload => "备份文件上传失败".into(),
            BackupFail::Data => "恢复数据出现错误".into(),
            BackupFail::Sql(msg) => format!("恢复过程中遇到如下错误: {msg}"),
        }
    }
}

pub struct RestoredSession {
    pub uid: i64,
    pub auth_hash: String,
}

struct Record {
    type_id: u16,
    rows: Option<Vec<(String, Option<String>)>>,
}

pub fn header_text() -> String {
    HEADER_MARK.replace("XXXX", VERSION)
}

pub fn safe_dat_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if name.len() < 5 || name.len() > 200 || !name.ends_with(".dat") {
        return None;
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        Some(name.to_string())
    } else {
        None
    }
}

pub fn list_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return names;
    };
    for entry in rd.flatten() {
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if entry.path().is_file() && safe_dat_name(&name).is_some() {
            names.push(name);
        }
    }
    names.sort();
    names
}

pub fn backup_page(dir: &Path, flash: &str) -> String {
    let files = list_files(dir);
    let path = html_escape(&dir.display().to_string());
    let (server, downloads) = if files.is_empty() {
        (
            format!(
                r#"<ul class="typecho-option"><li><p class="description">将备份文件手动上传至服务器的 {path} 目录下后, 这里会出现文件选项</p></li></ul>"#
            ),
            String::new(),
        )
    } else {
        let mut options = String::new();
        let mut extra = String::from("<ul class=\"typecho-option\">");
        for file in &files {
            let name = html_escape(file);
            options.push_str(&format!("<option value=\"{name}\">{name}</option>"));
            extra.push_str(&format!(
                "<li><a href=\"/index.php/action/backup?do=download&file={name}\">{name}</a> <form method=\"post\" action=\"/index.php/action/backup\" style=\"display:inline\"><input type=\"hidden\" name=\"do\" value=\"delete\"><input type=\"hidden\" name=\"file\" value=\"{name}\"><button type=\"submit\" class=\"btn btn-xs\">删除</button></form></li>"
            ));
        }
        extra.push_str("</ul>");
        (
            format!(
                r#"<ul class="typecho-option"><li><label class="typecho-label" for="backup-select-file">选择一个备份文件恢复数据</label><select name="file" id="backup-select-file">{options}</select></li></ul>"#
            ),
            extra,
        )
    };
    format!(
        r##"<div class="row typecho-page-main" role="main"><div class="col-mb-12 col-tb-8"><div id="typecho-welcome">{flash}<form action="/index.php/action/backup" method="post"><h3>备份您的数据</h3><ul><li>此备份操作仅包含<strong>内容数据</strong>, 并不会涉及任何<strong>设置信息</strong></li><li>如果您的数据量过大, 为了避免操作超时, 建议您直接使用数据库提供的备份工具备份数据</li><li><strong class="warning">为了缩小备份文件体积, 建议您在备份前删除不必要的数据</strong></li><li>导出默认只下载。勾选「同时在服务器留一份」后，会在备份目录留一份同名 .dat，可在「从服务器」下载或删除。</li></ul><p><label for="backup-retain"><input type="checkbox" id="backup-retain" name="retain" value="1"> 同时在服务器留一份</label></p><p><button class="btn primary" type="submit">开始备份 &raquo;</button></p><input type="hidden" name="do" value="export"></form></div></div><div id="backup-secondary" class="col-mb-12 col-tb-4" role="form"><h3>恢复数据</h3><ul class="typecho-option-tabs clearfix"><li class="active w-50"><a href="#from-upload">上传</a></li><li class="w-50"><a href="#from-server">从服务器</a></li></ul><form action="/index.php/action/backup" id="from-upload" class="tab-content backup-import" method="post" enctype="multipart/form-data"><ul class="typecho-option"><li><input id="backup-upload-file" name="file" type="file" class="file"></li></ul><ul class="typecho-option typecho-option-submit"><li><button type="submit" class="btn primary">上传并恢复 &raquo;</button><input type="hidden" name="do" value="import"></li></ul></form><div id="from-server" class="tab-content hidden"><form action="/index.php/action/backup" class="backup-import" method="post">{server}<ul class="typecho-option typecho-option-submit"><li><button type="submit" class="btn primary">选择并恢复 &raquo;</button><input type="hidden" name="do" value="import"></li></ul></form>{downloads}</div></div></div><script>
document.querySelectorAll('#backup-secondary .typecho-option-tabs a').forEach(function (link) {{
  link.addEventListener('click', function (event) {{
    event.preventDefault();
    document.querySelectorAll('#backup-secondary .typecho-option-tabs li').forEach(function (item) {{ item.classList.remove('active'); }});
    link.parentElement.classList.add('active');
    document.querySelectorAll('#backup-secondary .tab-content').forEach(function (pane) {{ pane.classList.add('hidden'); }});
    var pane = document.querySelector(link.getAttribute('href'));
    if (pane) pane.classList.remove('hidden');
  }});
}});
document.querySelectorAll('#backup-secondary form.backup-import').forEach(function (form) {{
  form.addEventListener('submit', function (event) {{
    if (!confirm('恢复操作将清除所有现有数据, 是否继续?')) event.preventDefault();
  }});
}});
</script>"##
    )
}

pub async fn export_bytes(db: &Db) -> Result<Vec<u8>, sqlx::Error> {
    let mut out = header_text().into_bytes();
    for (type_id, table) in [
        (1u16, "contents"),
        (2, "comments"),
        (3, "metas"),
        (4, "relationships"),
        (5, "users"),
        (6, "fields"),
    ] {
        let sql = select_sql(db, table);
        let rows = sqlx::query(&sql).fetch_all(&db.pool).await?;
        for row in &rows {
            let fields = columns(table)
                .iter()
                .map(|name| (*name, cell_text(row, name)))
                .collect::<Vec<_>>();
            out.extend(encode_record(type_id, &fields));
        }
    }
    out.extend(header_text().into_bytes());
    Ok(out)
}

pub fn export_filename(site: &Site, now: i64) -> String {
    let (y, m, d) = text::ymd(now, site.timezone());
    let host = site_host(site);
    let uniq = format!("{now:x}{:x}", std::process::id());
    format!("{y:04}{m:02}{d:02}_{host}_{uniq}.dat")
}

pub fn store_copy(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

pub fn read_stored(dir: &Path, name: &str) -> Result<Vec<u8>, BackupFail> {
    let Some(name) = safe_dat_name(name) else {
        return Err(BackupFail::Missing);
    };
    let path = dir.join(name);
    if !path.is_file() {
        return Err(BackupFail::Missing);
    }
    std::fs::read(&path).map_err(|_| BackupFail::Read)
}

pub fn delete_stored(dir: &Path, name: &str) -> Result<(), BackupFail> {
    let Some(name) = safe_dat_name(name) else {
        return Err(BackupFail::Missing);
    };
    let path = dir.join(name);
    if !path.is_file() {
        return Err(BackupFail::Missing);
    }
    std::fs::remove_file(path).map_err(|_| BackupFail::Read)
}

pub async fn import_bytes(db: &Db, bytes: &[u8], now: i64) -> Result<Option<RestoredSession>, BackupFail> {
    let records = parse_backup(bytes)?;
    let mut tx = db.pool.begin().await.map_err(|e| BackupFail::Sql(e.to_string()))?;
    let session = apply_records(db, &mut tx, &records, now)
        .await
        .map_err(|e| BackupFail::Sql(e.to_string()))?;
    tx.commit().await.map_err(|e| BackupFail::Sql(e.to_string()))?;
    Ok(session)
}

pub fn backup_dir(cfg: &Config) -> PathBuf {
    cfg.backup_dir.clone()
}

fn site_host(site: &Site) -> String {
    let raw = site.get("siteUrl");
    let host = url::Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "localhost".into());
    let host: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    if host.is_empty() {
        "localhost".into()
    } else {
        host
    }
}

fn columns(table: &str) -> &'static [&'static str] {
    match table {
        "contents" => CONTENTS,
        "comments" => COMMENTS,
        "metas" => METAS,
        "relationships" => RELATIONSHIPS,
        "users" => USERS,
        "fields" => FIELDS,
        _ => &[],
    }
}

fn table_of(type_id: u16) -> Option<&'static str> {
    match type_id {
        1 => Some("contents"),
        2 => Some("comments"),
        3 => Some("metas"),
        4 => Some("relationships"),
        5 => Some("users"),
        6 => Some("fields"),
        _ => None,
    }
}

fn select_sql(db: &Db, table: &str) -> String {
    let cols = columns(table)
        .iter()
        .map(|name| db.quote(name))
        .collect::<Vec<_>>()
        .join(", ");
    let order = match table {
        "contents" => format!(" ORDER BY {}", db.quote("cid")),
        "comments" => format!(" ORDER BY {}", db.quote("coid")),
        "metas" => format!(" ORDER BY {}", db.quote("mid")),
        "relationships" => format!(" ORDER BY {}, {}", db.quote("cid"), db.quote("mid")),
        "users" => format!(" ORDER BY {}", db.quote("uid")),
        "fields" => format!(" ORDER BY {}, {}", db.quote("cid"), db.quote("name")),
        _ => String::new(),
    };
    format!("SELECT {cols} FROM {}{order}", db.table(table))
}

fn cell_text(row: &sqlx::any::AnyRow, name: &str) -> Option<String> {
    if let Ok(value) = row.try_get::<Option<String>, _>(name) {
        return value;
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(name) {
        return value.map(|n| n.to_string());
    }
    if let Ok(value) = row.try_get::<Option<i32>, _>(name) {
        return value.map(|n| n.to_string());
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(name) {
        return value.map(num_text);
    }
    if let Ok(value) = row.try_get::<String, _>(name) {
        return Some(value);
    }
    if let Ok(value) = row.try_get::<i64, _>(name) {
        return Some(value.to_string());
    }
    if let Ok(value) = row.try_get::<f64, _>(name) {
        return Some(num_text(value));
    }
    None
}

fn num_text(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn encode_record(type_id: u16, fields: &[(&str, Option<String>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut json = String::from("{");
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push('"');
        json.push_str(key);
        json.push_str("\":");
        match value {
            None => json.push_str("null"),
            Some(text) => {
                json.push_str(&text.len().to_string());
                body.extend(text.as_bytes());
            }
        }
    }
    json.push('}');
    let header = json.into_bytes();
    let mut meta = Vec::with_capacity(8);
    meta.extend_from_slice(&type_id.to_le_bytes());
    meta.extend_from_slice(&(header.len() as u16).to_le_bytes());
    meta.extend_from_slice(&(body.len() as u32).to_le_bytes());
    let mut buf = meta.clone();
    buf.extend_from_slice(&header);
    buf.extend_from_slice(&body);
    buf.extend(auth::md5_hex(&buf).into_bytes());
    let _ = meta;
    buf
}

fn parse_header(bytes: &[u8]) -> Option<String> {
    if bytes.len() != HEADER_MARK.len() {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let matched = text.len() == HEADER_MARK.len()
        && text.starts_with("%TYPECHO_BACKUP_")
        && text.ends_with('%')
        && text.len() >= 21
        && text[16..20].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if matched {
        Some(text[16..20].to_string())
    } else {
        None
    }
}

fn parse_backup(bytes: &[u8]) -> Result<Vec<Record>, BackupFail> {
    let mark_len = HEADER_MARK.len();
    if bytes.len() < mark_len * 2 {
        return Err(BackupFail::Format);
    }
    let version = parse_header(&bytes[..mark_len]).ok_or(BackupFail::Format)?;
    let footer = parse_header(&bytes[bytes.len() - mark_len..]).ok_or(BackupFail::Format)?;
    if footer != version {
        return Err(BackupFail::Format);
    }
    let mut offset = mark_len;
    let end = bytes.len() - mark_len;
    let mut records = Vec::new();
    while offset < end {
        let (next, type_id, header, body) =
            read_record(bytes, offset, &version).map_err(|_| BackupFail::Data)?;
        if next > end {
            return Err(BackupFail::Data);
        }
        let rows = match table_of(type_id) {
            Some(_) => Some(split_row(&header, &body).map_err(|_| BackupFail::Data)?),
            None => None,
        };
        records.push(Record { type_id, rows });
        offset = next;
    }
    if offset != end {
        return Err(BackupFail::Data);
    }
    Ok(records)
}

fn read_record(data: &[u8], offset: usize, version: &str) -> Result<(usize, u16, Vec<u8>, Vec<u8>), ()> {
    let meta_len = if version == "FILE" { 6 } else { 8 };
    if data.len() < offset + meta_len {
        return Err(());
    }
    let meta = &data[offset..offset + meta_len];
    let mut cursor = offset + meta_len;
    let type_id = u16::from_le_bytes([meta[0], meta[1]]);
    let header_len = u16::from_le_bytes([meta[2], meta[3]]) as usize;
    if data.len() < cursor + header_len {
        return Err(());
    }
    let header = data[cursor..cursor + header_len].to_vec();
    cursor += header_len;
    let body_len = if version == "FILE" {
        json_body_len(&header)?
    } else {
        u32::from_le_bytes([meta[4], meta[5], meta[6], meta[7]]) as usize
    };
    if data.len() < cursor + body_len + 32 {
        return Err(());
    }
    let body = data[cursor..cursor + body_len].to_vec();
    cursor += body_len;
    let digest = &data[cursor..cursor + 32];
    cursor += 32;
    let mut hashed = Vec::with_capacity(meta_len + header.len() + body.len());
    hashed.extend_from_slice(meta);
    hashed.extend_from_slice(&header);
    hashed.extend_from_slice(&body);
    if digest != auth::md5_hex(&hashed).as_bytes() {
        return Err(());
    }
    Ok((cursor, type_id, header, body))
}

/// Typecho 的 `json_encode` 保留字段顺序。长度表只有字符串键，值是整数或 null。
fn parse_schema(header: &[u8]) -> Result<Vec<(String, Option<usize>)>, ()> {
    let text = std::str::from_utf8(header).map_err(|_| ())?.trim();
    if !text.starts_with('{') || !text.ends_with('}') {
        return Err(());
    }
    let inner = text[1..text.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        if bytes[i] != b'"' {
            return Err(());
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\\' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'"' {
            return Err(());
        }
        let key = std::str::from_utf8(&bytes[start..i]).map_err(|_| ())?.to_string();
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b':') {
            return Err(());
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if inner[i..].starts_with("null") {
            out.push((key, None));
            i += 4;
        } else {
            let num_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if num_start == i {
                return Err(());
            }
            let len = std::str::from_utf8(&bytes[num_start..i])
                .map_err(|_| ())?
                .parse::<usize>()
                .map_err(|_| ())?;
            out.push((key, Some(len)));
        }
    }
    Ok(out)
}

fn json_body_len(header: &[u8]) -> Result<usize, ()> {
    let mut total = 0usize;
    for (_, len) in parse_schema(header)? {
        if let Some(len) = len {
            total = total.checked_add(len).ok_or(())?;
        }
    }
    Ok(total)
}

fn split_row(header: &[u8], body: &[u8]) -> Result<Vec<(String, Option<String>)>, ()> {
    let mut offset = 0usize;
    let mut out = Vec::new();
    for (key, len) in parse_schema(header)? {
        let Some(len) = len else {
            out.push((key, None));
            continue;
        };
        if offset + len > body.len() {
            return Err(());
        }
        let text = std::str::from_utf8(&body[offset..offset + len]).map_err(|_| ())?;
        offset += len;
        out.push((key, Some(text.to_string())));
    }
    Ok(out)
}

async fn apply_records(
    db: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    records: &[Record],
    now: i64,
) -> Result<Option<RestoredSession>, sqlx::Error> {
    let mut cleared = HashSet::new();
    let mut relogin = false;
    let mut session = None;
    for record in records {
        let Some(table) = table_of(record.type_id) else {
            continue;
        };
        let Some(rows) = &record.rows else {
            continue;
        };
        if !cleared.contains(table) {
            let sql = format!("DELETE FROM {}", db.table(table));
            sqlx::query(&sql).execute(&mut **tx).await?;
            cleared.insert(table);
        }
        let mut data: HashMap<String, Option<String>> = rows.iter().cloned().collect();
        if !relogin && table == "users" && data.get("group").and_then(|v| v.as_deref()) == Some("administrator") {
            let auth = data
                .get("authCode")
                .and_then(|v| v.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{:016x}{:016x}", now.max(0) as u64, std::process::id() as u64));
            let uid = data
                .get("uid")
                .and_then(|v| v.as_deref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            data.insert("authCode".into(), Some(auth.clone()));
            data.insert("activated".into(), Some(now.to_string()));
            data.insert("logged".into(), Some(now.to_string()));
            session = Some(RestoredSession {
                uid,
                auth_hash: auth::typecho_hash(&auth, "tiebidli1"),
            });
            relogin = true;
        }
        insert_row(db, tx, table, &data).await?;
    }
    Ok(session)
}

async fn insert_row(
    db: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
    data: &HashMap<String, Option<String>>,
) -> Result<(), sqlx::Error> {
    let allowed = columns(table);
    let present: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|name| data.contains_key(*name))
        .collect();
    if present.is_empty() {
        return Ok(());
    }
    let cols = present
        .iter()
        .map(|name| db.quote(name))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = vec!["?"; present.len()].join(", ");
    let sql = format!("INSERT INTO {} ({cols}) VALUES ({marks})", db.table(table));
    let mut query = sqlx::query(&sql);
    for name in &present {
        query = query.bind(data.get(*name).cloned().unwrap_or(None));
    }
    query.execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_buffer_round_trip_keeps_nulls_and_utf8() {
        let fields = [
            ("cid", Some("1".into())),
            ("title", Some("你好".into())),
            ("template", None),
        ];
        let blob = encode_record(1, &fields);
        let header = header_text().into_bytes();
        let mut file = header.clone();
        file.extend_from_slice(&blob);
        file.extend_from_slice(&header);
        let records = parse_backup(&file).expect("parse");
        assert_eq!(records.len(), 1);
        let rows = records[0].rows.as_ref().unwrap();
        assert_eq!(rows[0], ("cid".into(), Some("1".into())));
        assert_eq!(rows[1], ("title".into(), Some("你好".into())));
        assert_eq!(rows[2], ("template".into(), None));
    }

    #[test]
    fn unknown_type_is_skipped_after_checksum() {
        let blob = encode_record(99, &[("plugin", Some("x".into()))]);
        let header = header_text().into_bytes();
        let mut file = header.clone();
        file.extend_from_slice(&blob);
        file.extend_from_slice(&header);
        let records = parse_backup(&file).expect("parse");
        assert!(records[0].rows.is_none());
    }
}
