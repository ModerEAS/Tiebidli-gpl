//! Sitemap 内置插件的数据层：查询已发布内容并渲染 `urlset` XML。
//!
//! 对齐参考 `Sitemap/Action.php`：取 `status=publish` 且 `created < now`
//! 的 `page`（priority 0.8）在前、`post`（priority 0.5）在后，
//! `changefreq=always`，`lastmod` 取 `modified` 的 `Y-m-d`，
//! 链接用现有路由反解后再套 `Options->index`，最后拼成绝对地址。

use crate::db::{col_i64, col_str, Db};
use crate::links;
use crate::model::{FieldValue, Post, Site};
use crate::text;
use std::collections::HashMap;

pub const XSL_PATH: &str = "/usr/plugins/Sitemap/sitemap.xsl";
pub const XSL_BODY: &str = include_str!("../static/plugins/Sitemap/sitemap.xsl");

/// 已发布且到期的 page + post，供 sitemap 逐条输出。
pub async fn entries(db: &Db, now: i64) -> Result<(Vec<Post>, Vec<Post>), sqlx::Error> {
    Ok((
        entries_of_kind(db, "page", now).await?,
        entries_of_kind(db, "post", now).await?,
    ))
}

async fn entries_of_kind(db: &Db, kind: &str, now: i64) -> Result<Vec<Post>, sqlx::Error> {
    let order = db.order_col();
    // 与公开站一致：只收 publish 且已到期，不收 hidden/draft/未来。
    let sql = format!(
        "SELECT c.cid, c.title, c.slug, c.created, c.modified, c.text, c.{order} AS sort_order, \
         c.authorId, c.template, c.type, c.status, c.password, c.commentsNum, \
         c.allowComment, c.allowPing, c.allowFeed, c.parent, \
         u.screenName AS screen_name, u.name AS user_name, u.mail AS user_mail, u.url AS user_url \
         FROM {} c LEFT JOIN {} u ON c.authorId = u.uid \
         WHERE c.type = ? AND c.status = 'publish' AND c.created < ? \
         ORDER BY c.created DESC, c.cid DESC",
        db.table("contents"),
        db.table("users"),
    );
    let rows = sqlx::query(&sql)
        .bind(kind)
        .bind(now)
        .fetch_all(&db.pool)
        .await?;
    let mut posts = Vec::with_capacity(rows.len());
    for row in &rows {
        posts.push(Post {
            cid: col_i64(row, "cid")?,
            title: col_str(row, "title"),
            slug: col_str(row, "slug"),
            created: col_i64(row, "created")?,
            modified: col_i64(row, "modified")?,
            text: col_str(row, "text"),
            sort_order: col_i64(row, "sort_order")?,
            author_id: col_i64(row, "authorId")?,
            template: col_str(row, "template"),
            kind: col_str(row, "type"),
            status: col_str(row, "status"),
            password: col_str(row, "password"),
            comments_num: col_i64(row, "commentsNum")?,
            allow_comment: crate::db::is_yes(&col_str(row, "allowComment")),
            allow_ping: crate::db::is_yes(&col_str(row, "allowPing")),
            allow_feed: crate::db::is_yes(&col_str(row, "allowFeed")),
            parent: col_i64(row, "parent")?,
            author_screen: col_str(row, "screen_name"),
            author_name: col_str(row, "user_name"),
            author_mail: col_str(row, "user_mail"),
            author_url: col_str(row, "user_url"),
            categories: Vec::new(),
            tags: Vec::new(),
            fields: HashMap::new(),
            views: 0,
        });
    }
    attach_categories(db, &mut posts).await?;
    // views 列若存在则顺手带上，不存在也不让 sitemap 失败。
    fill_views_best_effort(db, &mut posts).await;
    Ok(posts)
}

async fn attach_categories(db: &Db, posts: &mut [Post]) -> Result<(), sqlx::Error> {
    if posts.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = posts.iter().map(|p| p.cid).collect();
    let marks = vec!["?"; ids.len()].join(",");
    let order = db.order_col();
    let sql = format!(
        "SELECT r.cid AS rel_cid, m.mid, m.name, m.slug, m.type, m.description, m.count, \
         m.{order} AS sort_order, m.parent \
         FROM {} r JOIN {} m ON r.mid = m.mid WHERE r.cid IN ({marks}) AND m.type = 'category'",
        db.table("relationships"),
        db.table("metas"),
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let rows = q.fetch_all(&db.pool).await?;
    for row in &rows {
        let cid = col_i64(row, "rel_cid")?;
        let meta = crate::model::Meta {
            mid: col_i64(row, "mid")?,
            name: col_str(row, "name"),
            slug: col_str(row, "slug"),
            kind: col_str(row, "type"),
            description: col_str(row, "description"),
            count: col_i64(row, "count")?,
            sort_order: col_i64(row, "sort_order")?,
            parent: col_i64(row, "parent")?,
        };
        if let Some(post) = posts.iter_mut().find(|p| p.cid == cid) {
            post.categories.push(meta);
        }
    }
    for post in posts.iter_mut() {
        post.categories
            .sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.mid.cmp(&b.mid)));
    }
    Ok(())
}

async fn fill_views_best_effort(db: &Db, posts: &mut [Post]) {
    if posts.is_empty() {
        return;
    }
    let ids: Vec<i64> = posts.iter().map(|p| p.cid).collect();
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT cid, views FROM {} WHERE cid IN ({marks})",
        db.table("contents")
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(*id);
    }
    let Ok(rows) = q.fetch_all(&db.pool).await else {
        return;
    };
    for row in &rows {
        let cid = col_i64(row, "cid").unwrap_or(0);
        let views = col_i64(row, "views").unwrap_or(0).max(0);
        if let Some(post) = posts.iter_mut().find(|p| p.cid == cid) {
            post.views = views;
        }
    }
}

/// 渲染 `sitemap.xml`。`base` 为站点根（不带末尾斜杠）。
pub fn render(site: &Site, base: &str, pages: &[Post], posts: &[Post]) -> String {
    let xsl = links::abs(base, XSL_PATH);
    let tz = site.timezone();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(&format!(
        "<?xml-stylesheet type='text/xsl' href='{}'?>\n",
        xml_escape(&xsl)
    ));
    out.push_str("<urlset xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n");
    out.push_str("xsi:schemaLocation=\"http://www.sitemaps.org/schemas/sitemap/0.9 http://www.sitemaps.org/schemas/sitemap/0.9/sitemap.xsd\"\n");
    out.push_str("xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">");
    for page in pages {
        push_url(&mut out, site, base, page, "0.8", tz);
    }
    for post in posts {
        push_url(&mut out, site, base, post, "0.5", tz);
    }
    out.push_str("</urlset>");
    out
}

fn push_url(out: &mut String, site: &Site, base: &str, post: &Post, priority: &str, tz: i64) {
    // 路由反解失败时与参考一致回落到 `#`，避免输出错误地址。
    let path = site
        .routes
        .url(
            post.kind.as_str(),
            &links::post_values(site, post),
        )
        .unwrap_or_else(|| "#".into());
    let loc = if path == "#" {
        "#".to_string()
    } else {
        links::abs(base, &links::with_index(site, &path))
    };
    let lastmod = text::format_php_date("Y-m-d", post.modified, tz);
    out.push_str("\n\t<url>\n");
    out.push_str(&format!("\t\t<loc>{}</loc>\n", xml_escape(&loc)));
    out.push_str(&format!("\t\t<lastmod>{}</lastmod>\n", xml_escape(&lastmod)));
    out.push_str("\t\t<changefreq>always</changefreq>\n");
    out.push_str(&format!("\t\t<priority>{priority}</priority>\n"));
    out.push_str("\t</url>\n");
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[allow(dead_code)]
fn _fields_marker(_: &FieldValue) {}
