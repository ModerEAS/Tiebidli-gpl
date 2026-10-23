//! Links 内置插件的数据层：友链表 `{prefix}links` 的建表与 CRUD。
//!
//! 列对齐 `linksInstall`：`lid, name, url, sort, image, description, user, order`。
//! `sort` 取值 `ten/one/good/others`，分别表示全站/内页/推荐/失效。
//! 路由与表单 action 兼容 `/action/links-edit`（`do=insert/update/delete/sort`）。

use crate::auth;
use crate::db::{col_i64, col_str, Db, Dialect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendLink {
    pub lid: i64,
    pub name: String,
    pub url: String,
    pub sort: String,
    pub image: String,
    pub description: String,
    pub user: String,
    pub order: i64,
}

pub const SORTS: &[(&str, &str)] = &[
    ("ten", "全站链接"),
    ("one", "内页链接"),
    ("good", "推荐链接"),
    ("others", "失效链接"),
];

pub fn sort_label(sort: &str) -> &'static str {
    match sort {
        "ten" => "全站链接",
        "one" => "内页链接",
        "good" => "推荐链接",
        _ => "失效链接",
    }
}

pub fn normalize_sort(raw: &str) -> String {
    match raw {
        "ten" | "one" | "good" | "others" => raw.to_string(),
        _ => "ten".into(),
    }
}

/// 激活时建表，SQLite/MySQL 方言分支；已存在则直接通过。
pub async fn ensure_table(db: &Db) -> Result<(), sqlx::Error> {
    let t = db.table("links");
    let create = match db.dialect {
        Dialect::Sqlite => format!(
            "CREATE TABLE IF NOT EXISTS {t} (\
             lid INTEGER PRIMARY KEY, name VARCHAR(200) DEFAULT NULL, \
             url VARCHAR(200) DEFAULT NULL, sort VARCHAR(200) DEFAULT NULL, \
             image VARCHAR(200) DEFAULT NULL, description VARCHAR(200) DEFAULT NULL, \
             user VARCHAR(200) DEFAULT NULL, {order} INTEGER DEFAULT 0)",
            order = db.order_col(),
        ),
        Dialect::Mysql => format!(
            "CREATE TABLE IF NOT EXISTS {t} (\
             lid INT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(200) DEFAULT NULL, \
             url VARCHAR(200) DEFAULT NULL, sort VARCHAR(200) DEFAULT NULL, \
             image VARCHAR(200) DEFAULT NULL, description VARCHAR(200) DEFAULT NULL, \
             user VARCHAR(200) DEFAULT NULL, {order} INT DEFAULT 0)",
            order = db.order_col(),
        ),
    };
    sqlx::query(&create).execute(&db.pool).await?;
    Ok(())
}

pub async fn table_exists(db: &Db) -> bool {
    let probe = format!("SELECT lid FROM {} LIMIT 1", db.table("links"));
    sqlx::query(&probe).fetch_optional(&db.pool).await.is_ok()
}

pub async fn list(db: &Db) -> Result<Vec<FriendLink>, sqlx::Error> {
    let order = db.order_col();
    let sql = format!(
        "SELECT lid, name, url, sort, image, description, user, {order} AS link_order \
         FROM {} ORDER BY {order} ASC, lid ASC",
        db.table("links")
    );
    let rows = sqlx::query(&sql).fetch_all(&db.pool).await?;
    rows.iter().map(map_link).collect()
}

/// 供主题侧栏按 `sort` 输出，全站页用 `ten`，内页用 `one`，推荐用 `good`。
pub async fn list_by_sort(db: &Db, sort: &str) -> Result<Vec<FriendLink>, sqlx::Error> {
    let order = db.order_col();
    let sql = format!(
        "SELECT lid, name, url, sort, image, description, user, {order} AS link_order \
         FROM {} WHERE sort = ? ORDER BY {order} ASC, lid ASC",
        db.table("links")
    );
    let rows = sqlx::query(&sql)
        .bind(sort)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(map_link).collect()
}

pub async fn get(db: &Db, lid: i64) -> Result<Option<FriendLink>, sqlx::Error> {
    let order = db.order_col();
    let sql = format!(
        "SELECT lid, name, url, sort, image, description, user, {order} AS link_order \
         FROM {} WHERE lid = ? LIMIT 1",
        db.table("links")
    );
    let row = sqlx::query(&sql)
        .bind(lid)
        .fetch_optional(&db.pool)
        .await?;
    row.as_ref().map(map_link).transpose()
}

/// 校验表单，错误文案与参考表单一致。成功返回清洗后的字段。
pub fn validate(
    name: &str,
    url: &str,
    image: &str,
) -> Result<(String, String, String), &'static str> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    let image = image.trim().to_string();
    if name.is_empty() {
        return Err("必须填写链接名称");
    }
    if url.is_empty() {
        return Err("必须填写链接地址");
    }
    if !auth::url_ok(&url) {
        return Err("不是一个合法的链接地址");
    }
    if !image.is_empty() && !auth::url_ok(&image) {
        return Err("不是一个合法的图片地址");
    }
    Ok((name, url, image))
}

pub async fn insert(
    db: &Db,
    name: &str,
    url: &str,
    sort: &str,
    image: &str,
    description: &str,
    user: &str,
) -> Result<i64, sqlx::Error> {
    let order = db.order_col();
    let max: i64 = {
        let sql = format!("SELECT MAX({order}) AS n FROM {}", db.table("links"));
        let row = sqlx::query(&sql).fetch_optional(&db.pool).await?;
        row.as_ref()
            .map(|r| col_i64(r, "n").unwrap_or(0))
            .unwrap_or(0)
    };
    let sql = format!(
        "INSERT INTO {} (name, url, sort, image, description, user, {order}) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        db.table("links")
    );
    let res = sqlx::query(&sql)
        .bind(name)
        .bind(url)
        .bind(normalize_sort(sort))
        .bind(if image.is_empty() { None } else { Some(image) })
        .bind(if description.is_empty() {
            None
        } else {
            Some(description)
        })
        .bind(if user.is_empty() { None } else { Some(user) })
        .bind(max + 1)
        .execute(&db.pool)
        .await?;
    let mut id = res.last_insert_id().unwrap_or(0);
    if id == 0 && db.dialect == Dialect::Sqlite {
        let sql = format!("SELECT MAX(lid) AS n FROM {}", db.table("links"));
        let row = sqlx::query(&sql).fetch_optional(&db.pool).await?;
        id = row
            .as_ref()
            .map(|r| col_i64(r, "n").unwrap_or(0))
            .unwrap_or(0);
    }
    Ok(id)
}

pub async fn update(
    db: &Db,
    lid: i64,
    name: &str,
    url: &str,
    sort: &str,
    image: &str,
    description: &str,
    user: &str,
) -> Result<bool, sqlx::Error> {
    let sql = format!(
        "UPDATE {} SET name = ?, url = ?, sort = ?, image = ?, description = ?, user = ? \
         WHERE lid = ?",
        db.table("links")
    );
    let res = sqlx::query(&sql)
        .bind(name)
        .bind(url)
        .bind(normalize_sort(sort))
        .bind(if image.is_empty() { None } else { Some(image) })
        .bind(if description.is_empty() {
            None
        } else {
            Some(description)
        })
        .bind(if user.is_empty() { None } else { Some(user) })
        .bind(lid)
        .execute(&db.pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_many(db: &Db, lids: &[i64]) -> Result<u64, sqlx::Error> {
    if lids.is_empty() {
        return Ok(0);
    }
    let marks = vec!["?"; lids.len()].join(",");
    let sql = format!("DELETE FROM {} WHERE lid IN ({marks})", db.table("links"));
    let mut q = sqlx::query(&sql);
    for lid in lids {
        q = q.bind(*lid);
    }
    Ok(q.execute(&db.pool).await?.rows_affected())
}

/// 拖拽排序：按提交的 `lid[]` 先后顺序重写 `order`（从 1 开始）。
pub async fn apply_sort(db: &Db, lids: &[i64]) -> Result<(), sqlx::Error> {
    let order = db.order_col();
    for (index, lid) in lids.iter().enumerate() {
        let sql = format!("UPDATE {} SET {order} = ? WHERE lid = ?", db.table("links"));
        sqlx::query(&sql)
            .bind(index as i64 + 1)
            .bind(*lid)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

fn map_link(row: &sqlx::any::AnyRow) -> Result<FriendLink, sqlx::Error> {
    Ok(FriendLink {
        lid: col_i64(row, "lid")?,
        name: col_str(row, "name"),
        url: col_str(row, "url"),
        sort: {
            let s = col_str(row, "sort");
            if s.is_empty() {
                "ten".into()
            } else {
                s
            }
        },
        image: col_str(row, "image"),
        description: col_str(row, "description"),
        user: col_str(row, "user"),
        order: col_i64(row, "link_order").unwrap_or(0),
    })
}
