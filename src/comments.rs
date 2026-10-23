//! 评论分页与回响拆分，对齐 `Widget/Comments/Archive.php`。
//! 分页切的是顶层评论；子评论跟在当前页的父评论下面。

use crate::links;
use crate::model::{Comment, Post, Site};
use crate::text::html_escape;
use std::collections::HashMap;

pub struct CommentPage {
    pub comments: Vec<Comment>,
    pub pings: Vec<Comment>,
    pub page: i64,
    pub pages: i64,
    pub total_roots: i64,
}

pub fn page_of(site: &Site, comments: Vec<Comment>, requested: i64) -> CommentPage {
    let mut notes = Vec::new();
    let mut pings = Vec::new();
    for c in comments {
        if c.kind == "comment" {
            notes.push(c);
        } else {
            pings.push(c);
        }
    }
    let threaded = site.flag("commentsThreaded", true);
    let parents: HashMap<i64, i64> = notes.iter().map(|c| (c.coid, c.parent)).collect();
    let mut roots = Vec::new();
    for c in &notes {
        if threaded && c.parent != 0 && parents.contains_key(&c.parent) {
            continue;
        }
        roots.push(c.coid);
    }
    if site.get("commentsOrder").eq_ignore_ascii_case("DESC") {
        roots.reverse();
    }
    let total = roots.len() as i64;
    let break_on = site.flag("commentsPageBreak", false);
    let page_size = site.int("commentsPageSize", 20).max(1);
    let pages = if total == 0 {
        1
    } else if break_on {
        (total + page_size - 1) / page_size
    } else {
        1
    };
    let page = if !break_on {
        1
    } else if requested <= 0 && site.get("commentsPageDisplay") == "last" {
        pages
    } else {
        requested.max(1).min(pages)
    };
    let start = if break_on {
        ((page - 1) * page_size) as usize
    } else {
        0
    };
    let end = if break_on {
        (start + page_size as usize).min(roots.len())
    } else {
        roots.len()
    };
    let chosen: HashMap<i64, ()> = roots[start..end].iter().map(|id| (*id, ())).collect();
    let visible = notes
        .into_iter()
        .filter(|c| root_of(c.coid, &parents).is_some_and(|r| chosen.contains_key(&r)))
        .collect();
    CommentPage {
        comments: visible,
        pings,
        page,
        pages,
        total_roots: total,
    }
}

fn root_of(mut id: i64, parents: &HashMap<i64, i64>) -> Option<i64> {
    let mut guard = 0;
    while let Some(parent) = parents.get(&id).copied() {
        if parent == 0 || !parents.contains_key(&parent) {
            return Some(id);
        }
        id = parent;
        guard += 1;
        if guard > 32 {
            return Some(id);
        }
    }
    Some(id)
}

pub fn nav_html(site: &Site, post: &Post, page: &CommentPage) -> String {
    if !site.flag("commentsPageBreak", false) || page.total_roots <= site.int("commentsPageSize", 20).max(1)
    {
        return String::new();
    }
    let mut html = String::from("<ol class=\"page-navigator comment-page-nav\">");
    for n in 1..=page.pages {
        let path = links::comment_page_path(site, post, n);
        if n == page.page {
            html.push_str(&format!("<li class=\"current\"><span>{n}</span></li>"));
        } else {
            html.push_str(&format!(
                "<li><a href=\"{path}#comments\">{n}</a></li>"
            ));
        }
    }
    html.push_str("</ol>");
    html
}

pub fn ping_list_html(pings: &[Comment]) -> String {
    let mut html = String::new();
    for p in pings {
        let href = html_escape(&p.url);
        html.push_str(&format!(
            "<li id=\"comment-{}\" class=\"ping-item\"><span class=\"ping-author\"><a href=\"{href}\" rel=\"nofollow\">{}</a></span> <span class=\"ping-text\">{}</span></li>",
            p.coid,
            html_escape(&p.author),
            html_escape(&p.text)
        ));
    }
    html
}
