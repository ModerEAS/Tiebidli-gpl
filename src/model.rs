use crate::phpval::PhpValue;
use crate::router::{table_from_php, RouterTable};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FieldValue {
    pub str_value: String,
    pub int_value: i64,
}

#[derive(Debug, Clone)]
pub struct Meta {
    pub mid: i64,
    pub name: String,
    pub slug: String,
    pub kind: String,
    pub description: String,
    pub count: i64,
    pub sort_order: i64,
    pub parent: i64,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub cid: i64,
    pub title: String,
    pub slug: String,
    pub created: i64,
    pub modified: i64,
    pub text: String,
    pub sort_order: i64,
    pub author_id: i64,
    pub template: String,
    pub kind: String,
    pub status: String,
    pub password: String,
    pub comments_num: i64,
    pub allow_comment: bool,
    pub allow_ping: bool,
    pub allow_feed: bool,
    pub parent: i64,
    pub author_screen: String,
    pub author_name: String,
    pub author_mail: String,
    pub author_url: String,
    pub categories: Vec<Meta>,
    pub tags: Vec<Meta>,
    pub fields: HashMap<String, FieldValue>,
}

impl Post {
    pub fn display_author(&self) -> String {
        if !self.author_screen.is_empty() {
            self.author_screen.clone()
        } else if !self.author_name.is_empty() {
            self.author_name.clone()
        } else {
            "佚名".into()
        }
    }

    pub fn field_str(&self, name: &str) -> Option<&str> {
        self.fields
            .get(name)
            .map(|f| f.str_value.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn has_password(&self) -> bool {
        !self.password.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub coid: i64,
    pub cid: i64,
    pub created: i64,
    pub author: String,
    pub author_id: i64,
    pub owner_id: i64,
    pub mail: String,
    pub url: String,
    pub ip: String,
    pub text: String,
    pub kind: String,
    pub status: String,
    pub parent: i64,
}

#[derive(Debug, Clone)]
pub struct Site {
    pub opts: HashMap<String, String>,
    pub theme: PhpValue,
    pub routes: RouterTable,
    pub categories: Vec<Meta>,
    pub tags: Vec<Meta>,
    pub pages: Vec<Post>,
}

impl Site {
    pub fn get(&self, key: &str) -> &str {
        self.opts.get(key).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        let v = self.get(key);
        if v.is_empty() {
            default
        } else {
            v
        }
    }

    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            "" => default,
            "0" | "false" | "off" | "no" => false,
            _ => true,
        }
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        let v = self.get(key);
        if v.is_empty() {
            default
        } else {
            v.parse().unwrap_or(default)
        }
    }

    pub fn theme_str(&self, key: &str) -> String {
        self.theme
            .get_key(key)
            .and_then(|v| v.to_string_value())
            .unwrap_or_default()
    }

    pub fn theme_list(&self, key: &str) -> Vec<String> {
        self.theme
            .get_key(key)
            .map(|v| v.string_list())
            .unwrap_or_default()
    }

    pub fn page_size(&self) -> i64 {
        let n = self.int("pageSize", 5);
        if n <= 0 {
            5
        } else {
            n
        }
    }

    pub fn timezone(&self) -> i64 {
        self.int("timezone", 28_800)
    }

    pub fn theme_name(&self) -> String {
        theme_directory_name(self.get("theme"))
    }
}

pub fn theme_directory_name(name: &str) -> String {
    let ok = !name.is_empty()
        && name.len() <= 24
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        name.to_string()
    } else {
        "default".into()
    }
}

pub fn site_from_parts(
    opts: HashMap<String, String>,
    categories: Vec<Meta>,
    tags: Vec<Meta>,
    pages: Vec<Post>,
) -> Site {
    let theme_key = format!("theme:{}", theme_directory_name(opts.get("theme").map(String::as_str).unwrap_or("")));
    let theme = opts
        .get(&theme_key)
        .and_then(|raw| crate::phpval::parse_php(raw).ok())
        .unwrap_or(PhpValue::Null);
    let routes = opts
        .get("routingTable")
        .and_then(|raw| crate::phpval::parse_php(raw).ok())
        .map(|v| table_from_php(&v))
        .unwrap_or_else(crate::router::default_table);
    Site {
        opts,
        theme,
        routes,
        categories,
        tags,
        pages,
    }
}
