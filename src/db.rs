use sqlx::any::{Any, AnyPoolOptions, AnyRow};
use sqlx::{Pool, Row};
use std::sync::Once;

static DRIVERS: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Mysql,
    Sqlite,
}

impl Dialect {
    pub fn parse(url: &str) -> anyhow::Result<Self> {
        if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            Ok(Dialect::Mysql)
        } else if url.starts_with("sqlite:") {
            Ok(Dialect::Sqlite)
        } else {
            anyhow::bail!("DATABASE_URL 需要 mysql:// 或 sqlite: 。PostgreSQL 不在这一版的运行时里")
        }
    }
}

#[derive(Clone)]
pub struct Db {
    pub pool: Pool<Any>,
    pub prefix: String,
    pub dialect: Dialect,
}

impl Db {
    pub async fn connect(url: &str, prefix: &str) -> anyhow::Result<Self> {
        let dialect = Dialect::parse(url)?;
        DRIVERS.call_once(|| {
            sqlx::any::install_default_drivers();
        });
        let opts =
            AnyPoolOptions::new().max_connections(if dialect == Dialect::Sqlite { 1 } else { 8 });
        let pool = opts.connect(url).await?;
        Ok(Self {
            pool,
            prefix: prefix.to_string(),
            dialect,
        })
    }

    pub fn table(&self, name: &str) -> String {
        let ident = format!("{}{name}", self.prefix);
        self.quote(&ident)
    }

    pub fn quote(&self, ident: &str) -> String {
        match self.dialect {
            Dialect::Mysql => format!("`{ident}`"),
            Dialect::Sqlite => format!("\"{ident}\""),
        }
    }

    pub fn order_col(&self) -> String {
        self.quote("order")
    }

    pub fn random_fn(&self) -> &'static str {
        match self.dialect {
            Dialect::Mysql => "RAND()",
            Dialect::Sqlite => "RANDOM()",
        }
    }
}

pub fn col_i64(row: &AnyRow, name: &str) -> Result<i64, sqlx::Error> {
    if let Ok(v) = row.try_get::<i64, _>(name) {
        return Ok(v);
    }
    if let Ok(v) = row.try_get::<i32, _>(name) {
        return Ok(i64::from(v));
    }
    if let Ok(v) = row.try_get::<i16, _>(name) {
        return Ok(i64::from(v));
    }
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(name) {
        return Ok(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(name) {
        return Ok(i64::from(v));
    }
    if let Ok(v) = row.try_get::<String, _>(name) {
        return Ok(v.parse().unwrap_or(0));
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(name) {
        return Ok(v.parse().unwrap_or(0));
    }
    let _ = name;
    Ok(0)
}

pub fn col_str(row: &AnyRow, name: &str) -> String {
    if let Ok(v) = row.try_get::<String, _>(name) {
        return v;
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(name) {
        return v.unwrap_or_default();
    }
    if let Ok(v) = row.try_get::<i64, _>(name) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<i32, _>(name) {
        return v.to_string();
    }
    String::new()
}

pub fn is_yes(s: &str) -> bool {
    matches!(s, "1" | "true" | "yes")
}

/// 让 `Any` 出现在类型里，避免某些工具链把驱动特征裁掉。
#[allow(dead_code)]
fn _dialect_marker() -> Option<Any> {
    None
}
