//! Typecho 1.2 的密码和 cookie 前缀。
//! cookie 名是 `md5(rootUrl)` 接上键名，`rootUrl` 不带末尾斜杠。
//! 密码：`$P$` 走 phpass，`$T$` 走 `Common::hash`，其余按 md5 明文比较。

use crate::db::{col_i64, col_str, Db};
use md5::{Digest, Md5};

const ITOA64: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

#[derive(Debug, Clone)]
pub struct SessionUser {
    pub uid: i64,
    pub name: String,
    pub screen: String,
    pub mail: String,
    pub url: String,
    pub group: String,
    pub logged: i64,
    /// 库里的明文 `authCode`。`Security::getToken` 用它，cookie 里存的是它的摘要。
    pub auth_code: String,
}

impl SessionUser {
    pub fn pass(&self, group: &str) -> bool {
        rank(&self.group) <= rank(group)
    }

    pub fn label(&self) -> String {
        if self.screen.is_empty() {
            self.name.clone()
        } else {
            self.screen.clone()
        }
    }
}

pub fn rank(group: &str) -> i32 {
    match group {
        "administrator" => 0,
        "editor" => 1,
        "contributor" => 2,
        "subscriber" => 3,
        _ => 4,
    }
}

pub fn md5_hex(bytes: &[u8]) -> String {
    hex::encode(Md5::digest(bytes))
}

pub fn cookie_prefix(root_url: &str) -> String {
    md5_hex(root_url.trim_end_matches('/').as_bytes())
}

pub fn cookie_name(root_url: &str, key: &str) -> String {
    format!("{}{key}", cookie_prefix(root_url))
}

/// `Widget\Security::getToken`：`md5(secret [& authCode & uid] & suffix)`。
/// 未登录时材料是 `secret + '&' + suffix`；已登录再插入 `authCode` 和 `uid`。
pub fn security_token(secret: &str, user: Option<(&str, i64)>, suffix: &str) -> String {
    let mut base = secret.to_string();
    if let Some((code, uid)) = user {
        base.push('&');
        base.push_str(code);
        base.push('&');
        base.push_str(&uid.to_string());
    }
    base.push('&');
    base.push_str(suffix);
    md5_hex(base.as_bytes())
}

pub fn site_root(configured: &Option<String>, site_url: &str) -> String {
    let raw = configured
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(site_url);
    raw.trim().trim_end_matches('/').to_string()
}

/// `Common::hashValidate`。`from` 是原文，`to` 是库存或 cookie 里的摘要。
pub fn hash_validate(from: &str, to: &str) -> bool {
    if let Some(rest) = to.strip_prefix("$T$") {
        if rest.len() < 9 {
            return false;
        }
        typecho_hash(from, &rest[..9]) == to
    } else {
        md5_hex(from.as_bytes()) == to
    }
}

pub fn typecho_hash(input: &str, salt: &str) -> String {
    let bytes = input.as_bytes();
    let salt_b = salt.as_bytes();
    if bytes.is_empty() || salt_b.len() != 9 {
        return String::new();
    }
    let mut last = i32::from(bytes[bytes.len() - 1]);
    let mut hash = Vec::with_capacity(bytes.len());
    for &asc_b in bytes {
        let asc = i32::from(asc_b);
        if asc == 0 {
            return String::new();
        }
        let idx = ((last % asc).rem_euclid(9)) as usize;
        last = (last * i32::from(salt_b[idx]) + asc).rem_euclid(95) + 32;
        hash.push(last as u8);
    }
    format!("$T${salt}{}", md5_hex(&hash))
}

pub fn phpass_check(password: &str, stored: &str) -> bool {
    if !stored.starts_with("$P$") {
        return false;
    }
    crypt_private(password, stored) == stored
}

/// Typecho `PasswordHash(8, true)`：`$P$`，迭代次数 2^8。
pub fn hash_password(password: &str) -> String {
    let stamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let mix = md5_hex(format!("{stamp}:{password}:{}", password.len()).as_bytes());
    let mut salt = String::with_capacity(8);
    for byte in mix.bytes().take(8) {
        salt.push(ITOA64[(byte as usize) % 64] as char);
    }
    let setting = format!("$P${}{salt}", ITOA64[8] as char);
    crypt_private(password, &format!("{setting}........"))
}

/// `Validate::xssCheck`。空字符串通过，调用方自己处理必填。
pub fn xss_ok(input: &str) -> bool {
    let decoded = decode_xss_entities(input);
    !decoded.chars().any(|c| {
        let n = c as u32;
        matches!(c, '(' | ')' | '\\' | '"' | '<' | '>' | '\r' | '\n' | '\t')
            || n <= 0x08
            || n == 0x0b
            || n == 0x0c
            || (0x0e..=0x19).contains(&n)
    })
}

/// `Validate::email`：过滤后的地址必须和原文一致，并且能当邮箱用。
pub fn email_ok(input: &str) -> bool {
    if input.is_empty() || sanitize_email(input) != input {
        return false;
    }
    let Some((local, domain)) = input.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.len() < 3 || !domain.contains('.') {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }
    let local_ok = local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c));
    let domain_ok = domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c));
    local_ok && domain_ok
}

/// `Validate::url`。空字符串留给「非必填则跳过」的规则。
pub fn url_ok(input: &str) -> bool {
    if input.is_empty() {
        return true;
    }
    if input
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\\'))
    {
        return false;
    }
    match url::Url::parse(input) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https") && parsed.host().is_some(),
        Err(_) => false,
    }
}

fn sanitize_email(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-=?^_`{|}~@.[]".contains(*c))
        .collect()
}

fn decode_xss_entities(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' && i + 2 < chars.len() && chars[i + 1] == '#' {
            let hex = chars[i + 2] == 'x' || chars[i + 2] == 'X';
            let start = if hex { i + 3 } else { i + 2 };
            let mut j = start;
            let mut zeros = 0usize;
            while j < chars.len() && chars[j] == '0' && zeros < 8 {
                zeros += 1;
                j += 1;
            }
            let num_start = j;
            while j < chars.len() && (if hex { chars[j].is_ascii_hexdigit() } else { chars[j].is_ascii_digit() }) {
                j += 1;
            }
            if j > num_start {
                let digits: String = chars[num_start..j].iter().collect();
                let code = if hex {
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    digits.parse::<u32>().ok()
                };
                if let Some(code) = code.and_then(char::from_u32) {
                    if j < chars.len() && chars[j] == ';' {
                        j += 1;
                    }
                    out.push(code);
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn password_ok(password: &str, stored: &str) -> bool {
    if stored.starts_with("$P$") {
        phpass_check(password, stored)
    } else {
        hash_validate(password, stored)
    }
}

fn crypt_private(password: &str, setting: &str) -> String {
    let bytes = setting.as_bytes();
    if bytes.len() < 12 || &bytes[..3] != b"$P$" {
        return "*0".into();
    }
    let Some(count_log2) = ITOA64.iter().position(|c| *c == bytes[3]) else {
        return "*0".into();
    };
    if !(7..=30).contains(&count_log2) {
        return "*0".into();
    }
    let mut count = 1u32 << count_log2;
    let salt = &setting[4..12];
    let mut hash = Md5::digest(format!("{salt}{password}").as_bytes()).to_vec();
    while count > 0 {
        count -= 1;
        let mut data = hash.clone();
        data.extend(password.as_bytes());
        hash = Md5::digest(&data).to_vec();
    }
    format!("{}{}", &setting[..12], encode64(&hash, 16))
}

fn encode64(input: &[u8], count: usize) -> String {
    let mut output = String::new();
    let mut i = 0usize;
    while i < count {
        let mut value = input[i] as u32;
        i += 1;
        output.push(ITOA64[(value & 0x3f) as usize] as char);
        if i < count {
            value |= (input[i] as u32) << 8;
        }
        output.push(ITOA64[((value >> 6) & 0x3f) as usize] as char);
        i += 1;
        if i >= count {
            break;
        }
        if i < count {
            value |= (input[i] as u32) << 16;
        }
        output.push(ITOA64[((value >> 12) & 0x3f) as usize] as char);
        i += 1;
        if i >= count {
            break;
        }
        output.push(ITOA64[((value >> 18) & 0x3f) as usize] as char);
    }
    output
}

pub async fn login_user(db: &Db, name: &str, password: &str) -> Result<Option<SessionUser>, sqlx::Error> {
    let column = if name.contains('@') { "mail" } else { "name" };
    let group = db.quote("group");
    let sql = format!(
        "SELECT uid, name, password, mail, url, screenName, {group} AS user_group, authCode, logged \
         FROM {} WHERE {column} = ? LIMIT 1",
        db.table("users")
    );
    let row = sqlx::query(&sql).bind(name).fetch_optional(&db.pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !password_ok(password, &col_str(&row, "password")) {
        return Ok(None);
    }
    Ok(Some(user_from_row(&row)))
}

pub async fn user_by_uid(db: &Db, uid: i64) -> Result<Option<SessionUser>, sqlx::Error> {
    let group = db.quote("group");
    let sql = format!(
        "SELECT uid, name, mail, url, screenName, {group} AS user_group, authCode, logged \
         FROM {} WHERE uid = ? LIMIT 1",
        db.table("users")
    );
    let row = sqlx::query(&sql).bind(uid).fetch_optional(&db.pool).await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn session_user(
    db: &Db,
    root: &str,
    cookies: &std::collections::HashMap<String, String>,
) -> Result<Option<SessionUser>, sqlx::Error> {
    let uid: i64 = cookies
        .get(&cookie_name(root, "__typecho_uid"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if uid <= 0 {
        return Ok(None);
    }
    let code = cookies
        .get(&cookie_name(root, "__typecho_authCode"))
        .cloned()
        .unwrap_or_default();
    let group = db.quote("group");
    let sql = format!(
        "SELECT uid, name, mail, url, screenName, {group} AS user_group, authCode, logged \
         FROM {} WHERE uid = ? LIMIT 1",
        db.table("users")
    );
    let row = sqlx::query(&sql).bind(uid).fetch_optional(&db.pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored = col_str(&row, "authCode");
    if stored.is_empty() || !hash_validate(&stored, &code) {
        return Ok(None);
    }
    Ok(Some(user_from_row(&row)))
}

pub async fn commit_login(db: &Db, uid: i64) -> Result<String, sqlx::Error> {
    let auth_code = format!("{:032x}{:08x}", uid, chrono::Utc::now().timestamp().max(0) as u64);
    let sql = format!(
        "UPDATE {} SET authCode = ?, logged = activated WHERE uid = ?",
        db.table("users")
    );
    sqlx::query(&sql)
        .bind(&auth_code)
        .bind(uid)
        .execute(&db.pool)
        .await?;
    Ok(typecho_hash(&auth_code, "tiebidli1"))
}

fn user_from_row(row: &sqlx::any::AnyRow) -> SessionUser {
    SessionUser {
        uid: col_i64(row, "uid").unwrap_or(0),
        name: col_str(row, "name"),
        screen: col_str(row, "screenName"),
        mail: col_str(row, "mail"),
        url: col_str(row, "url"),
        group: col_str(row, "user_group"),
        logged: col_i64(row, "logged").unwrap_or(0),
        auth_code: col_str(row, "authCode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phpass_accepts_its_own_digest() {
        let setting = "$P$7abcdefgh";
        let digest = crypt_private("s3cret", &format!("{setting}ignored-tail"));
        assert!(digest.starts_with(setting));
        assert!(phpass_check("s3cret", &digest));
        assert!(!phpass_check("other", &digest));
    }

    #[test]
    fn hash_password_uses_phpass_and_roundtrips() {
        let stored = hash_password("adminpass");
        assert!(stored.starts_with("$P$6"), "{stored}");
        assert!(phpass_check("adminpass", &stored));
        assert!(!phpass_check("adminpas", &stored));
    }

    #[test]
    fn profile_rules_match_typecho_validate() {
        assert!(xss_ok("站长"));
        assert!(!xss_ok("<b>站长</b>"));
        assert!(!xss_ok("&#60;b&#62;"));
        assert!(email_ok("admin@example.com"));
        assert!(!email_ok("not-an-email"));
        assert!(url_ok(""));
        assert!(url_ok("https://example.com/path"));
        assert!(!url_ok("javascript:alert(1)"));
    }

    #[test]
    fn md5_password_matches_typecho_fallback() {
        let stored = md5_hex(b"adminpass");
        assert!(password_ok("adminpass", &stored));
        assert!(!password_ok("nope", &stored));
    }

    #[test]
    fn cookie_prefix_is_md5_of_root_without_slash() {
        let a = cookie_prefix("http://test.local");
        let b = cookie_prefix("http://test.local/");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert_eq!(
            cookie_name("http://test.local", "protectPassword_3"),
            format!("{a}protectPassword_3")
        );
    }
}
