//! 文章正文：Typecho 的 `<!--markdown-->` / `<!--more-->`、摘要、日期。
//! 时区按 `typecho_options.timezone`（秒）把 `created` 的 Unix 时间显示成站点墙钟。
//! v1.2 写入时 `created` 是真实 Unix 时间（见 `Widget/Contents/Post/Edit.php::getCreated`）。

use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};
use pulldown_cmark::{html, Options, Parser};

pub fn split_markdown(raw: &str) -> (bool, &str) {
    if let Some(rest) = raw.strip_prefix("<!--markdown-->") {
        (true, rest)
    } else {
        (false, raw)
    }
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn render_post_html(raw: &str) -> String {
    let (is_md, body) = split_markdown(raw);
    let html = if is_md {
        markdown_to_html(body)
    } else {
        auto_p(body)
    };
    clean_post(&html)
}

pub fn plain_text(raw: &str) -> String {
    let (is_md, body) = split_markdown(raw);
    let body = body.replace("<!--more-->", "\n");
    let html = if is_md { markdown_to_html(&body) } else { body };
    collapse_ws(&strip_tags(&html))
}

/// 列表摘要：有 `<!--more-->` 时只用前半；否则截断到 `limit` 个字符。
pub fn excerpt(raw: &str, limit: usize) -> String {
    let (is_md, body) = split_markdown(raw);
    let (head, _) = body.split_once("<!--more-->").unwrap_or((body, ""));
    let html = if is_md {
        markdown_to_html(head)
    } else {
        head.to_string()
    };
    let plain = collapse_ws(&strip_tags(&html));
    truncate_chars(&plain, limit)
}

/// 列表正文：有 `<!--more-->` 时只渲染前半，并加上默认主题的「阅读剩余部分」。
pub fn list_html(raw: &str, permalink: &str, title: &str) -> String {
    let (is_md, body) = split_markdown(raw);
    let Some((head, _)) = body.split_once("<!--more-->") else {
        return render_post_html(raw);
    };
    let html = if is_md {
        markdown_to_html(head)
    } else {
        auto_p(head)
    };
    format!(
        "{html}<p class=\"more\"><a href=\"{permalink}\" title=\"{title}\">- 阅读剩余部分 -</a></p>",
        html = clean_post(&html),
        permalink = html_escape(permalink),
        title = html_escape(title),
    )
}

pub fn word_count(raw: &str) -> usize {
    plain_text(raw)
        .chars()
        .filter(|c| !c.is_whitespace())
        .count()
}

pub fn first_image(raw: &str, field_thumb: Option<&str>) -> Option<String> {
    if let Some(thumb) = field_thumb {
        let thumb = thumb.trim();
        if thumb.is_empty() || thumb == "no" {
            if thumb == "no" {
                return None;
            }
        } else {
            return Some(thumb.to_string());
        }
    }
    let (_, body) = split_markdown(raw);
    if let Some(c) = img_src(body) {
        return Some(c);
    }
    None
}

fn img_src(body: &str) -> Option<String> {
    let md = RegexHolder::markdown_img();
    if let Some(c) = md.captures(body) {
        return Some(c[1].to_string());
    }
    let html = RegexHolder::html_img();
    html.captures(body).map(|c| c[1].to_string())
}

struct RegexHolder;

impl RegexHolder {
    fn markdown_img() -> &'static regex::Regex {
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| regex::Regex::new(r"!\[[^\]]*\]\(([^)\s]+)").expect("md img"))
    }
    fn html_img() -> &'static regex::Regex {
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)<img[^>]+src\s*=\s*["']([^"']+)["']"#).expect("html img")
        })
    }
}

fn markdown_to_html(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(src, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

fn auto_p(text: &str) -> String {
    let mut html = String::new();
    for para in text.split("\n\n") {
        let inner = para.trim();
        if inner.is_empty() {
            continue;
        }
        if inner.starts_with('<') {
            html.push_str(inner);
            html.push('\n');
        } else {
            html.push_str("<p>");
            html.push_str(&inner.replace('\n', "<br>\n"));
            html.push_str("</p>\n");
        }
    }
    html
}

fn clean_post(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.add_tags([
        "span",
        "div",
        "figure",
        "figcaption",
        "iframe",
        "video",
        "source",
        "sup",
        "sub",
        "kbd",
    ]);
    b.add_tag_attributes(
        "iframe",
        ["src", "width", "height", "allow", "allowfullscreen"],
    );
    b.add_tag_attributes("video", ["src", "controls", "width", "height", "poster"]);
    b.add_tag_attributes("source", ["src", "type"]);
    b.add_generic_attributes(["class", "id", "style"]);
    b.url_relative(ammonia::UrlRelative::PassThrough);
    b.clean(html).to_string()
}

pub fn clean_comment(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.tags(std::collections::HashSet::from([
        "p",
        "br",
        "a",
        "code",
        "pre",
        "strong",
        "em",
        "ul",
        "ol",
        "li",
        "blockquote",
        "del",
    ]));
    b.add_tag_attributes("a", ["href", "title"]);
    b.rm_tag_attributes("a", ["rel"]);
    b.url_relative(ammonia::UrlRelative::PassThrough);
    b.link_rel(Some("nofollow noopener"));
    b.clean(html).to_string()
}

pub fn render_comment_html(raw: &str, markdown: bool) -> String {
    if markdown {
        clean_comment(&markdown_to_html(raw))
    } else {
        let escaped = html_escape(raw);
        clean_comment(&auto_p(&escaped))
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

pub fn truncate_chars(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let mut out: String = s.chars().take(limit).collect();
    out.push_str("...");
    out
}

pub fn trunc_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub fn offset(secs: i64) -> FixedOffset {
    let secs = secs.clamp(-86_400, 86_400) as i32;
    FixedOffset::east_opt(secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}

pub fn format_php_date(fmt: &str, ts: i64, tz_secs: i64) -> String {
    let tz = offset(tz_secs);
    let Some(dt) = tz.timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
            continue;
        }
        match c {
            'Y' => out.push_str(&format!("{:04}", dt.year())),
            'y' => out.push_str(&format!("{:02}", dt.year().rem_euclid(100))),
            'm' => out.push_str(&format!("{:02}", dt.month())),
            'n' => out.push_str(&dt.month().to_string()),
            'd' => out.push_str(&format!("{:02}", dt.day())),
            'j' => out.push_str(&dt.day().to_string()),
            'H' => out.push_str(&format!("{:02}", dt.hour())),
            'G' => out.push_str(&dt.hour().to_string()),
            'h' => {
                let h = dt.hour() % 12;
                out.push_str(&format!("{:02}", if h == 0 { 12 } else { h }));
            }
            'i' => out.push_str(&format!("{:02}", dt.minute())),
            's' => out.push_str(&format!("{:02}", dt.second())),
            'a' => out.push_str(if dt.hour() < 12 { "am" } else { "pm" }),
            'A' => out.push_str(if dt.hour() < 12 { "AM" } else { "PM" }),
            'S' => out.push_str(ordinal(dt.day())),
            'F' => out.push_str(month_full(dt.month())),
            'M' => out.push_str(month_short(dt.month())),
            _ => out.push(c),
        }
    }
    out
}

fn ordinal(d: u32) -> &'static str {
    if (11..=13).contains(&(d % 100)) {
        "th"
    } else {
        match d % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    }
}

fn month_full(m: u32) -> &'static str {
    [
        "",
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(m as usize)
    .copied()
    .unwrap_or("")
}

fn month_short(m: u32) -> &'static str {
    [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(m as usize)
    .copied()
    .unwrap_or("")
}

pub fn iso8601(ts: i64, tz_secs: i64) -> String {
    let tz = offset(tz_secs);
    let Some(dt) = tz.timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let sign = if tz_secs >= 0 { '+' } else { '-' };
    let abs = tz_secs.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        abs / 3600,
        (abs % 3600) / 60,
    )
}

pub fn ymd(ts: i64, tz_secs: i64) -> (i32, u32, u32) {
    let tz = offset(tz_secs);
    if let Some(dt) = tz.timestamp_opt(ts, 0).single() {
        (dt.year(), dt.month(), dt.day())
    } else {
        (1970, 1, 1)
    }
}

/// 站点时区下某年/月/日的 `[start, end)` Unix 区间。月、日为 0 表示不限定。
pub fn calendar_range(year: i32, month: u32, day: u32, tz_secs: i64) -> Option<(i64, i64)> {
    let tz = offset(tz_secs);
    if month == 0 {
        let start = tz.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single()?;
        let end = tz.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).single()?;
        return Some((start.timestamp(), end.timestamp()));
    }
    if day == 0 {
        let start_date = NaiveDate::from_ymd_opt(year, month, 1)?;
        let end_date = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let start = tz
            .with_ymd_and_hms(
                start_date.year(),
                start_date.month(),
                start_date.day(),
                0,
                0,
                0,
            )
            .single()?;
        let end = tz
            .with_ymd_and_hms(end_date.year(), end_date.month(), end_date.day(), 0, 0, 0)
            .single()?;
        return Some((start.timestamp(), end.timestamp()));
    }
    let start_date = NaiveDate::from_ymd_opt(year, month, day)?;
    let end_date = start_date.succ_opt()?;
    let start = tz.with_ymd_and_hms(year, month, day, 0, 0, 0).single()?;
    let end = tz
        .with_ymd_and_hms(end_date.year(), end_date.month(), end_date.day(), 0, 0, 0)
        .single()?;
    Some((start.timestamp(), end.timestamp()))
}

pub fn snippet_around(plain: &str, keyword: &str, radius: usize) -> String {
    let lower_src: String = plain.chars().flat_map(|c| c.to_lowercase()).collect();
    let lower_kw: String = keyword.chars().flat_map(|c| c.to_lowercase()).collect();
    let chars: Vec<char> = plain.chars().collect();
    let start = if lower_kw.is_empty() {
        0
    } else {
        lower_src
            .find(&lower_kw)
            .map(|byte| lower_src[..byte].chars().count())
            .unwrap_or(0)
    };
    let from = start.saturating_sub(radius);
    let to = (start + lower_kw.chars().count() + radius).min(chars.len());
    let mut out: String = chars[from..to].iter().collect();
    if from > 0 {
        out.insert_str(0, "...");
    }
    if to < chars.len() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_tag_and_markdown() {
        let raw = "<!--markdown-->\n# 标题\n\n可见\n\n<!--more-->\n\n隐藏";
        let ex = excerpt(raw, 100);
        assert!(ex.contains("可见"));
        assert!(!ex.contains("隐藏"));
        let html = render_post_html(raw);
        assert!(html.contains("<h1>"));
        assert!(html.contains("隐藏"));
    }

    #[test]
    fn php_date_respects_offset() {
        assert_eq!(format_php_date("Y-m-d", 0, 0), "1970-01-01");
        assert_eq!(format_php_date("Y-m-d H:i", 0, 28800), "1970-01-01 08:00");
    }

    #[test]
    fn calendar_nov_2023_utc() {
        let (start, end) = calendar_range(2023, 11, 14, 0).unwrap();
        assert!(start <= 1_700_000_000 && 1_700_000_000 < end);
    }
}
