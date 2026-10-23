use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub prefix: String,
    pub site_url: Option<String>,
    pub listen: SocketAddr,
    pub theme_dir: PathBuf,
    /// 外部模板目录。同名 html 在启动时覆盖内置模板。
    pub template_dir: Option<PathBuf>,
    pub uploads_dir: Option<PathBuf>,
    pub static_dir: PathBuf,
    pub gravatar_prefix: String,
    /// 备份文件目录，对应原版 `__TYPECHO_BACKUP_DIR__`。默认 `./usr/backups`。
    pub backup_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy_load();
        let database_url = std::env::var("DATABASE_URL").map_err(|_| {
            anyhow::anyhow!("缺少 DATABASE_URL。示例：mysql://user:pass@127.0.0.1:3306/typecho")
        })?;
        let prefix = std::env::var("TYPECHO_PREFIX").unwrap_or_else(|_| "typecho_".into());
        validate_prefix(&prefix)?;
        let site_url = std::env::var("SITE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let listen = std::env::var("LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|e| anyhow::anyhow!("LISTEN 不是合法地址: {e}"))?;
        let theme_dir = PathBuf::from(
            std::env::var("THEME_STATIC_DIR").unwrap_or_else(|_| "./themes/default".into()),
        );
        let template_dir = std::env::var("THEME_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        let uploads_dir = std::env::var("UPLOADS_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        let static_dir =
            PathBuf::from(std::env::var("STATIC_DIR").unwrap_or_else(|_| "./static".into()));
        let gravatar_prefix = std::env::var("GRAVATAR_PREFIX")
            .unwrap_or_else(|_| "https://secure.gravatar.com/avatar/".into());
        let backup_dir =
            PathBuf::from(std::env::var("BACKUP_DIR").unwrap_or_else(|_| "./usr/backups".into()));
        Ok(Self {
            database_url,
            prefix,
            site_url,
            listen,
            theme_dir,
            template_dir,
            uploads_dir,
            static_dir,
            gravatar_prefix,
            backup_dir,
        })
    }

    pub fn for_test(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
            prefix: "typecho_".into(),
            site_url: Some("http://test.local".into()),
            listen: "127.0.0.1:0".parse().unwrap(),
            theme_dir: PathBuf::from("/tmp/tiebidli-no-theme"),
            template_dir: None,
            uploads_dir: None,
            static_dir: PathBuf::from("static"),
            gravatar_prefix: "https://secure.gravatar.com/avatar/".into(),
            backup_dir: PathBuf::from("/tmp/tiebidli-no-backup"),
        }
    }
}

pub fn validate_prefix(prefix: &str) -> anyhow::Result<()> {
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        anyhow::bail!("TYPECHO_PREFIX 只能包含字母、数字和下划线");
    }
    Ok(())
}

fn dotenvy_load() -> Option<()> {
    let path = std::path::Path::new(".env");
    if !path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=')?;
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        if std::env::var_os(k).is_none() {
            std::env::set_var(k, v);
        }
    }
    Some(())
}
