//! Typecho 1.2 路由表。规则与 `var/Typecho/Router/Parser.php`（v1.2.1）一致。
//! 默认表来自 `install.php` 的 `install_get_default_routers()`。

use crate::phpval::PhpValue;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Route {
    pub name: String,
    pub source: String,
    pub regex: Regex,
    pub params: Vec<String>,
    pub format: String,
}

#[derive(Debug, Clone)]
pub struct RouterTable {
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone)]
pub struct RouteMatch {
    pub name: String,
    pub params: HashMap<String, String>,
}

impl RouterTable {
    pub fn compile(pairs: &[(&str, &str)]) -> Self {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(n, u)| ((*n).to_string(), (*u).to_string()))
            .collect();
        Self::compile_owned(owned)
    }

    pub fn compile_owned(pairs: Vec<(String, String)>) -> Self {
        let routes = pairs
            .into_iter()
            .map(|(name, source)| compile_route(name, source))
            .collect();
        Self { routes }
    }

    pub fn url(&self, name: &str, values: &HashMap<String, String>) -> Option<String> {
        let route = self.routes.iter().find(|r| r.name == name)?;
        let mut out = route.format.clone();
        for p in &route.params {
            let v = values.get(p).map(|s| s.as_str()).unwrap_or("");
            if let Some(idx) = out.find("%s") {
                out.replace_range(idx..idx + 2, v);
            }
        }
        Some(out)
    }

    /// 按路由表顺序返回所有命中。Typecho 在处理函数抛 404 时会继续尝试下一条。
    pub fn all_matches(&self, path: &str) -> Vec<RouteMatch> {
        let mut out = Vec::new();
        for route in &self.routes {
            if let Some(caps) = route.regex.captures(path) {
                let mut params = HashMap::new();
                for (i, name) in route.params.iter().enumerate() {
                    let val = caps.get(i + 1).map(|m| m.as_str()).unwrap_or("");
                    params.insert(name.clone(), val.to_string());
                }
                out.push(RouteMatch {
                    name: route.name.clone(),
                    params,
                });
            }
        }
        out
    }
}

pub fn default_table() -> RouterTable {
    RouterTable::compile(&[
        ("index", "/"),
        ("archive", "/blog/"),
        ("do", "/action/[action:alpha]"),
        ("post", "/archives/[cid:digital]/"),
        ("attachment", "/attachment/[cid:digital]/"),
        ("category", "/category/[slug]/"),
        ("tag", "/tag/[slug]/"),
        ("author", "/author/[uid:digital]/"),
        ("search", "/search/[keywords]/"),
        ("index_page", "/page/[page:digital]/"),
        ("archive_page", "/blog/page/[page:digital]/"),
        ("category_page", "/category/[slug]/[page:digital]/"),
        ("tag_page", "/tag/[slug]/[page:digital]/"),
        ("author_page", "/author/[uid:digital]/[page:digital]/"),
        ("search_page", "/search/[keywords]/[page:digital]/"),
        ("archive_year", "/[year:digital:4]/"),
        ("archive_month", "/[year:digital:4]/[month:digital:2]/"),
        (
            "archive_day",
            "/[year:digital:4]/[month:digital:2]/[day:digital:2]/",
        ),
        (
            "archive_year_page",
            "/[year:digital:4]/page/[page:digital]/",
        ),
        (
            "archive_month_page",
            "/[year:digital:4]/[month:digital:2]/page/[page:digital]/",
        ),
        (
            "archive_day_page",
            "/[year:digital:4]/[month:digital:2]/[day:digital:2]/page/[page:digital]/",
        ),
        (
            "comment_page",
            "[permalink:string]/comment-page-[commentPage:digital]",
        ),
        ("feed", "/feed[feed:string:0]"),
        ("feedback", "[permalink:string]/[type:alpha]"),
        ("page", "/[slug].html"),
    ])
}

/// 用库里的 `routingTable` 覆盖默认同名规则的 `url`，保留 1.2 的匹配顺序。
pub fn table_from_php(value: &PhpValue) -> RouterTable {
    let base = default_table();
    let PhpValue::Array(items) = value else {
        return base;
    };
    let mut pairs: Vec<(String, String)> = base
        .routes
        .iter()
        .map(|r| (r.name.clone(), r.source.clone()))
        .collect();
    for (k, val) in items {
        let PhpValue::String(name) = k else {
            continue;
        };
        let Some(url) = val.get_key("url").and_then(|u| u.as_str()) else {
            continue;
        };
        if url.is_empty() {
            continue;
        }
        if let Some(slot) = pairs.iter_mut().find(|(n, _)| n == name) {
            slot.1 = url.to_string();
        } else {
            pairs.push((name.clone(), url.to_string()));
        }
    }
    RouterTable::compile_owned(pairs)
}

fn compile_route(name: String, source: String) -> Route {
    let (regex, params, format) = compile_pattern(&source);
    Route {
        name,
        source,
        regex,
        params,
        format,
    }
}

fn compile_pattern(url: &str) -> (Regex, Vec<String>, String) {
    let mut replaced = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '[' | ']' => replaced.push('%'),
            ':' => replaced.push(' '),
            _ => replaced.push(c),
        }
    }
    let quoted = regex_quote(&replaced);
    let mut params = Vec::new();
    let token = Regex::new(r"%([^%]+)%").expect("token regex");
    let expanded = token
        .replace_all(&quoted, |caps: &regex::Captures| {
            let inner = &caps[1];
            let parts: Vec<&str> = inner.split(' ').filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                return caps[0].to_string();
            }
            params.push(parts[0].to_string());
            match parts.len() {
                1 => typed_regex("char", "+"),
                2 => typed_regex(parts[1], "+"),
                3 => {
                    let n: i64 = parts[2].parse().unwrap_or(0);
                    let q = if n > 0 {
                        format!("{{{n}}}")
                    } else {
                        "*".into()
                    };
                    typed_regex(parts[1], &q)
                }
                4 => typed_regex(parts[1], &format!("{{{},{}}}", parts[2], parts[3])),
                _ => caps[0].to_string(),
            }
        })
        .into_owned();
    let expanded = expanded.trim_end_matches('/').to_string();
    let full = format!("^{expanded}/?$");
    let format_re = Regex::new(r"\[[^\]]+\]").expect("format");
    let format = format_re.replace_all(url, "%s").into_owned();
    let regex = Regex::new(&full).unwrap_or_else(|_| Regex::new("^$").expect("empty"));
    (regex, params, format)
}

fn typed_regex(ty: &str, quant: &str) -> String {
    match ty {
        "string" => format!("(.{quant})"),
        "digital" => format!("([0-9]{quant})"),
        "alpha" => format!("([_0-9a-zA-Z-]{quant})"),
        "alphaslash" => format!("([_0-9a-zA-Z-/]{quant})"),
        "split" => format!("((?:[^/]+/){quant}[^/]+)"),
        _ => format!("([^/]{quant})"),
    }
}

fn regex_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(path: &str) -> String {
        default_table()
            .all_matches(path)
            .into_iter()
            .next()
            .map(|m| m.name)
            .unwrap_or_default()
    }

    #[test]
    fn default_routes_match_typecho_order() {
        // 靠后的规则（尤其是 feedback）可能同时命中，Typecho 采用第一条能渲染的规则。
        assert_eq!(first("/"), "index");
        assert_eq!(first("/page/2"), "index_page");
        assert_eq!(first("/archives/12"), "post");
        assert_eq!(first("/category/notes"), "category");
        assert_eq!(first("/tag/rust/3"), "tag_page");
        assert_eq!(first("/search/hello"), "search");
        assert_eq!(first("/2024/01/"), "archive_month");
        assert_eq!(first("/about.html"), "page");
        let feed = default_table().all_matches("/feed/atom").remove(0);
        assert_eq!(feed.name, "feed");
        assert_eq!(feed.params.get("feed").map(String::as_str), Some("/atom"));
        let fb = default_table()
            .all_matches("/archives/12/comment")
            .into_iter()
            .find(|m| m.name == "feedback")
            .unwrap();
        assert_eq!(fb.params.get("type").map(String::as_str), Some("comment"));
    }

    #[test]
    fn builds_post_url_from_overridden_pattern() {
        let raw = r#"a:1:{s:4:"post";a:1:{s:3:"url";s:21:"/archives/[slug].html";}}"#;
        let table = table_from_php(&crate::phpval::parse_php(raw).unwrap());
        let mut vals = HashMap::new();
        vals.insert("slug".into(), "hello-world".into());
        vals.insert("cid".into(), "1".into());
        assert_eq!(
            table.url("post", &vals).as_deref(),
            Some("/archives/hello-world.html")
        );
        assert_eq!(names_of(&table, "/archives/hello-world.html"), vec!["post"]);
    }

    #[test]
    fn permalink_presets_generate_and_match() {
        let cases = [
            (
                "/archives/[cid:digital]/",
                &[("cid", "12")][..],
                "/archives/12/",
            ),
            (
                "/archives/[slug].html",
                &[("slug", "hello")],
                "/archives/hello.html",
            ),
            (
                "/[year:digital:4]/[month:digital:2]/[day:digital:2]/[slug].html",
                &[("year", "2024"), ("month", "01"), ("day", "02"), ("slug", "hello")],
                "/2024/01/02/hello.html",
            ),
            (
                "/[category]/[slug].html",
                &[("category", "notes"), ("slug", "hello")],
                "/notes/hello.html",
            ),
            (
                "/[directory:split:0]/[slug].html",
                &[("directory", "a/b"), ("slug", "hello")],
                "/a/b/hello.html",
            ),
            ("/[slug].html", &[("slug", "about")], "/about.html"),
            (
                "/category/[directory:split:0]/",
                &[("directory", "journal/travel")],
                "/category/journal/travel/",
            ),
        ];
        for (pattern, pairs, path) in cases {
            let table = RouterTable::compile(&[("custom", pattern)]);
            let vals = pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            assert_eq!(table.url("custom", &vals).as_deref(), Some(path), "{pattern}");
            let hit = table.all_matches(path);
            assert_eq!(hit.len(), 1, "{pattern} <- {path}");
            for (k, v) in pairs {
                assert_eq!(hit[0].params.get(*k).map(String::as_str), Some(*v), "{pattern} {k}");
            }
            let bare = path.trim_end_matches('/');
            if bare != path {
                assert_eq!(table.all_matches(bare).len(), 1, "optional slash {pattern}");
            }
        }
    }

    fn names_of(table: &RouterTable, path: &str) -> Vec<String> {
        table
            .all_matches(path)
            .into_iter()
            .map(|m| m.name)
            .collect()
    }
}
