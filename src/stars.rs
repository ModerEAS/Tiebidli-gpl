//! 主题点赞：`action=star_post&cid=` / `action=star_talk&coid=`。
//!
//! 对齐主题 `Star::starCommon`：计数存在 `contents.stars` / `comments.stars`
//!（主题建表时补的列，老库没有则 best-effort 补上）；同一个人靠 cookie
//! `extend_post_stars` / `extend_say_stars` 去重（逗号分隔 id）。
//! 响应是纯文本：`1` 成功、`2` 已赞过、`-1` 参数/库失败。

use crate::db::{col_i64, Db};

/// 点赞计数列，老库缺列时补上（与 `access::ensure_views_column` 同模式）。
pub async fn ensure_stars_columns(db: &Db) {
    for table in [db.table("contents"), db.table("comments")] {
        let probe = format!("SELECT stars FROM {table} LIMIT 1");
        if sqlx::query(&probe).fetch_optional(&db.pool).await.is_ok() {
            continue;
        }
        let alter = format!("ALTER TABLE {table} ADD COLUMN stars INTEGER DEFAULT 0");
        let _ = sqlx::query(&alter).execute(&db.pool).await;
    }
}

/// 当前评论点赞数（列缺失时回空 map，不阻塞渲染）。
pub async fn comment_stars(db: &Db, cid: i64) -> std::collections::HashMap<i64, i64> {
    let mut out = std::collections::HashMap::new();
    let sql = format!("SELECT coid, stars FROM {} WHERE cid = ?", db.table("comments"));
    let Ok(rows) = sqlx::query(&sql).bind(cid).fetch_all(&db.pool).await else {
        return out;
    };
    for row in &rows {
        if let (Ok(coid), Ok(stars)) = (col_i64(row, "coid"), col_i64(row, "stars")) {
            out.insert(coid, stars.max(0));
        }
    }
    out
}

/// 当前点赞数（列缺失/无行时 0）。
pub async fn star_count(db: &Db, table: &str, id_col: &str, id: i64) -> i64 {
    let sql = format!("SELECT stars FROM {table} WHERE {id_col} = ?");
    let Ok(Some(row)) = sqlx::query(&sql).bind(id).fetch_optional(&db.pool).await else {
        return 0;
    };
    col_i64(&row, "stars").unwrap_or(0).max(0)
}

/// id 不存在返回 `None`（回 `-1`）；否则 +1 返回 `true`。
pub async fn star_once(db: &Db, table: &str, id_col: &str, id: i64) -> Option<bool> {
    let sql = format!("SELECT stars FROM {table} WHERE {id_col} = ?");
    sqlx::query(&sql).bind(id).fetch_optional(&db.pool).await.ok()??;
    let sql = format!("UPDATE {table} SET stars = COALESCE(stars, 0) + 1 WHERE {id_col} = ?");
    sqlx::query(&sql).bind(id).execute(&db.pool).await.ok()?;
    Some(true)
}

/// cookie 里是否已赞过该 id。
pub fn already_starred(cookie: &str, id: i64) -> bool {
    let needle = id.to_string();
    cookie.split(',').any(|part| part.trim() == needle)
}

/// 把 id 追加进 cookie（调用方负责 Set-Cookie，最多保留 500 个）。
pub fn mark_starred(cookie: &str, id: i64) -> String {
    let mut ids: Vec<String> = cookie
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if !ids.iter().any(|item| item == &id.to_string()) {
        ids.push(id.to_string());
    }
    if ids.len() > 500 {
        ids = ids.split_off(ids.len() - 500);
    }
    ids.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starred_cookie_round_trip() {
        assert!(!already_starred("", 3));
        let next = mark_starred("", 3);
        assert!(already_starred(&next, 3));
        assert!(!already_starred(&next, 4));
        assert!(already_starred(&mark_starred("1,2", 3), 2));
    }
}
