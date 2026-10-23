use crate::links::{self, abs};
use crate::model::{Comment, Post, Site};
use crate::text::{self, html_escape};

#[derive(Clone, Copy)]
pub enum FeedKind {
    Rss,
    Atom,
    /// Typecho `/feed/rss`，RSS 1.0 RDF。
    Rdf,
}

pub fn render_posts(
    kind: FeedKind,
    site: &Site,
    base: &str,
    title: &str,
    description: &str,
    self_path: &str,
    posts: &[Post],
    full: bool,
) -> String {
    let items: Vec<Item> = posts
        .iter()
        .map(|p| {
            let path = links::content_path(site, p);
            let html = if full {
                text::render_post_html(&p.text)
            } else {
                let ex = text::excerpt(&p.text, 280);
                format!(
                    "<p>{}</p><p class=\"more\"><a href=\"{}\">[...]</a></p>",
                    html_escape(&ex),
                    html_escape(&abs(base, &path))
                )
            };
            Item {
                title: p.title.clone(),
                link: abs(base, &path),
                html,
                author: p.display_author(),
                created: p.created,
            }
        })
        .collect();
    render(kind, base, title, description, self_path, &items)
}

pub fn render_comments(
    kind: FeedKind,
    site: &Site,
    base: &str,
    title: &str,
    self_path: &str,
    rows: &[(Comment, Post)],
    markdown: bool,
) -> String {
    let items: Vec<Item> = rows
        .iter()
        .map(|(c, p)| {
            let path = format!("{}#comment-{}", links::content_path(site, p), c.coid);
            Item {
                title: c.author.clone(),
                link: abs(base, &path),
                html: text::render_comment_html(&c.text, markdown),
                author: c.author.clone(),
                created: c.created,
            }
        })
        .collect();
    render(kind, base, title, "", self_path, &items)
}

struct Item {
    title: String,
    link: String,
    html: String,
    author: String,
    created: i64,
}

fn render(
    kind: FeedKind,
    base: &str,
    title: &str,
    description: &str,
    self_path: &str,
    items: &[Item],
) -> String {
    match kind {
        FeedKind::Rss => rss(base, title, description, self_path, items),
        FeedKind::Atom => atom(base, title, description, self_path, items),
        FeedKind::Rdf => rdf(base, title, description, self_path, items),
    }
}

fn rdf(base: &str, title: &str, description: &str, self_path: &str, items: &[Item]) -> String {
    let about = abs(base, self_path);
    let mut body = String::new();
    body.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    body.push_str(
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/">"#,
    );
    body.push_str(&format!("<channel rdf:about=\"{}\">", html_escape(&about)));
    body.push_str(&format!("<title>{}</title>", html_escape(title)));
    body.push_str(&format!("<link>{}</link>", html_escape(base)));
    body.push_str(&format!(
        "<description>{}</description>",
        html_escape(description)
    ));
    body.push_str("<items><rdf:Seq>");
    for item in items {
        body.push_str(&format!(
            "<rdf:li resource=\"{}\"/>",
            html_escape(&item.link)
        ));
    }
    body.push_str("</rdf:Seq></items></channel>");
    for item in items {
        body.push_str(&format!(
            "<item rdf:about=\"{}\">",
            html_escape(&item.link)
        ));
        body.push_str(&format!("<title>{}</title>", html_escape(&item.title)));
        body.push_str(&format!("<link>{}</link>", html_escape(&item.link)));
        body.push_str(&format!(
            "<dc:date>{}</dc:date>",
            html_escape(&rfc3339(item.created))
        ));
        body.push_str(&format!(
            "<description>{}</description>",
            html_escape(&strip_tags(&item.html))
        ));
        body.push_str("</item>");
    }
    body.push_str("</rdf:RDF>");
    body
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if !in_tag {
            out.push(c);
        }
    }
    out
}

fn rss(base: &str, title: &str, description: &str, self_path: &str, items: &[Item]) -> String {
    let mut body = String::new();
    body.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    body.push_str(r#"<rss version="2.0"><channel>"#);
    body.push_str(&format!("<title>{}</title>", html_escape(title)));
    body.push_str(&format!("<link>{}</link>", html_escape(base)));
    body.push_str(&format!(
        "<description>{}</description>",
        html_escape(description)
    ));
    body.push_str(&format!(
        "<atom:link xmlns:atom=\"http://www.w3.org/2005/Atom\" href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>",
        html_escape(&abs(base, self_path))
    ));
    for item in items {
        body.push_str("<item>");
        body.push_str(&format!("<title>{}</title>", html_escape(&item.title)));
        body.push_str(&format!("<link>{}</link>", html_escape(&item.link)));
        body.push_str(&format!(
            "<guid isPermaLink=\"true\">{}</guid>",
            html_escape(&item.link)
        ));
        body.push_str(&format!(
            "<pubDate>{}</pubDate>",
            html_escape(&rfc2822(item.created))
        ));
        body.push_str(&format!(
            "<dc:creator xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{}</dc:creator>",
            html_escape(&item.author)
        ));
        body.push_str(&format!(
            "<description><![CDATA[{}]]></description>",
            item.html.replace("]]>", "]]&gt;")
        ));
        body.push_str("</item>");
    }
    body.push_str("</channel></rss>");
    body
}

fn atom(base: &str, title: &str, description: &str, self_path: &str, items: &[Item]) -> String {
    let mut body = String::new();
    body.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    body.push_str(r#"<feed xmlns="http://www.w3.org/2005/Atom">"#);
    body.push_str(&format!("<title>{}</title>", html_escape(title)));
    body.push_str(&format!(
        "<subtitle>{}</subtitle>",
        html_escape(description)
    ));
    body.push_str(&format!(
        "<link href=\"{}\" rel=\"alternate\"/>",
        html_escape(base)
    ));
    body.push_str(&format!(
        "<link href=\"{}\" rel=\"self\"/>",
        html_escape(&abs(base, self_path))
    ));
    body.push_str(&format!("<id>{}</id>", html_escape(&abs(base, self_path))));
    body.push_str(&format!("<updated>{}</updated>", rfc3339(chrono_now())));
    for item in items {
        body.push_str("<entry>");
        body.push_str(&format!("<title>{}</title>", html_escape(&item.title)));
        body.push_str(&format!(
            "<link href=\"{}\" rel=\"alternate\"/>",
            html_escape(&item.link)
        ));
        body.push_str(&format!("<id>{}</id>", html_escape(&item.link)));
        body.push_str(&format!("<updated>{}</updated>", rfc3339(item.created)));
        body.push_str(&format!(
            "<author><name>{}</name></author>",
            html_escape(&item.author)
        ));
        body.push_str(&format!(
            "<content type=\"html\">{}</content>",
            html_escape(&item.html)
        ));
        body.push_str("</entry>");
    }
    body.push_str("</feed>");
    body
}

fn chrono_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn rfc2822(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap())
        .to_rfc2822()
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap())
        .to_rfc3339()
}
