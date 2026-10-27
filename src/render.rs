use crate::config::Config;
use crate::links::{self, abs};
use crate::model::{Comment, Post, Site};
use crate::text::{self, html_escape};
use minijinja::{Environment, Error};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct Chrome<'a> {
    pub cfg: &'a Config,
    pub site: &'a Site,
    pub theme_ready: bool,
    pub recent_posts: &'a [Post],
    pub archives: &'a [(i32, u32, i64)],
    pub recent: &'a [(Comment, Post)],
    pub hot_posts: &'a [Post],
    pub random_posts: &'a [Post],
    pub friend_links: &'a [crate::friend_links::FriendLink],
    pub accept_language: Option<&'a str>,
    pub stats: &'a crate::repo::Stats,
    pub viewer_is_editor: bool,
    pub now: i64,
    pub cookies: HashMap<String, String>,
    pub viewer_name: Option<&'a str>,
    pub viewer_uid: i64,
    pub viewer_auth: &'a str,
    pub page_url: &'a str,
    /// 页脚打点用的归档 id：文章/页面为 `(Some(cid), None)`，分类/标签为 `(None, Some(mid))`，
    /// 首页为 `(None, Some(0))`，其余 `(None, None)`。对齐 `Access_Core::parseArchive`。
    pub access_ids: (Option<i64>, Option<i64>),
}

const EMBEDDED: &[(&str, &str)] = &[
    ("base.html", include_str!("../templates/base.html")),
    ("list.html", include_str!("../templates/list.html")),
    ("single.html", include_str!("../templates/single.html")),
    ("search.html", include_str!("../templates/search.html")),
    ("404.html", include_str!("../templates/404.html")),
];

/// `template_dir` 里的同名 html 覆盖内置模板。进程启动时读入，之后不热更新。
pub fn environment(template_dir: Option<&std::path::Path>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            minijinja::AutoEscape::Html
        } else {
            minijinja::AutoEscape::None
        }
    });
    for (name, embedded) in EMBEDDED {
        let src: &'static str = match template_dir {
            Some(dir) => match std::fs::read_to_string(dir.join(name)) {
                Ok(text) => Box::leak(text.into_boxed_str()),
                Err(_) => embedded,
            },
            None => embedded,
        };
        env.add_template(name, src).expect(name);
    }
    env
}

pub fn render_list(
    env: &Environment<'_>,
    chrome: &Chrome<'_>,
    heading: &str,
    sub: &str,
    posts: &[Post],
    page: i64,
    total: i64,
    base_route: &str,
    page_route: &str,
    route_vals: &HashMap<String, String>,
    show_tag_cloud: bool,
) -> Result<String, Error> {
    let cards: Vec<Value> = posts.iter().map(|p| post_card(chrome, p)).collect();
    let pagination = links::pagination(page, total, chrome.site.page_size(), |n| {
        links::paged_path(chrome.site, base_route, page_route, route_vals.clone(), n)
    });
    let mut ctx = shell(
        chrome,
        &format_title(heading, chrome, page),
        sub,
        show_tag_cloud,
        &[],
    );
    ctx["heading"] = json!(heading);
    ctx["sub"] = json!(sub);
    ctx["posts"] = json!(cards);
    ctx["pagination"] = json!(pagination);
    ctx["show_archive_title"] = json!(base_route != "index");
    ctx["nav_index"] = json!(base_route == "index");
    env.get_template("list.html")?.render(ctx)
}

pub fn render_search(
    env: &Environment<'_>,
    chrome: &Chrome<'_>,
    keyword: &str,
    posts: &[Post],
    page: i64,
    total: i64,
) -> Result<String, Error> {
    let cards: Vec<Value> = posts
        .iter()
        .map(|p| {
            let mut card = post_card(chrome, p);
            let plain = text::plain_text(&p.text);
            card["summary"] = json!(text::snippet_around(&plain, keyword, 60));
            card
        })
        .collect();
    let mut vals = HashMap::new();
    vals.insert("keywords".into(), links::enc(keyword));
    let pagination = links::pagination(page, total, chrome.site.page_size(), |n| {
        links::paged_path(chrome.site, "search", "search_page", vals.clone(), n)
    });
    let title = format!("包含关键字 {keyword} 的文章");
    let mut ctx = shell(chrome, &format_title(&title, chrome, page), "", false, &[]);
    ctx["keyword"] = json!(keyword);
    ctx["count"] = json!(total);
    ctx["posts"] = json!(cards);
    ctx["pagination"] = json!(pagination);
    env.get_template("search.html")?.render(ctx)
}

pub struct SingleView<'a> {
    pub post: &'a Post,
    pub locked: bool,
    pub password_error: &'a str,
    pub comment_error: &'a str,
    pub comments: &'a [Comment],
    pub newer: Option<&'a Post>,
    pub older: Option<&'a Post>,
    pub remember_author: &'a str,
    pub remember_mail: &'a str,
    pub remember_url: &'a str,
    pub remember_text: &'a str,
    pub reply_to: i64,
    pub comment_nav: &'a str,
    pub ping_html: &'a str,
    pub attachment_html: &'a str,
    pub stars: i64,
    pub cross_json: Option<Value>,
}

/// 时光机页评论流（`nav_slug == "cross"` 时模板用它代替默认评论树）。
/// 对齐主题源码：只收录登录用户（`author_id != 0`，过滤游客）；每条含点赞数；
/// 作者本人或编辑可得 `edit_url`（`_`-token 绑定本页 URL）。
/// `[secret]…[/secret]` 私密说说：登录可见原文，未登录只看占位。
pub async fn cross_stream_json(
    db: &crate::db::Db,
    chrome: &Chrome<'_>,
    post: &Post,
    comments: &[Comment],
) -> Value {
    let stars = crate::stars::comment_stars(db, post.cid).await;
    let markdown = chrome.site.flag("commentsMarkdown", false);
    let logged = chrome.viewer_uid != 0;
    let mut stream: Vec<&Comment> = comments.iter().filter(|c| c.author_id != 0).collect();
    // 最新在上：按 created/coid 降序（comments_for 是 ASC，展示侧反转）。
    stream.sort_by(|a, b| (b.created, b.coid).cmp(&(a.created, a.coid)));
    let items: Vec<Value> = stream
        .iter()
        .map(|c| {
            let can_edit = chrome.viewer_uid != 0
                && (c.author_id == chrome.viewer_uid || chrome.viewer_is_editor);
            let edit_url = if can_edit {
                let token = crate::auth::security_token(
                    chrome.site.get("secret"),
                    Some((chrome.viewer_auth, chrome.viewer_uid)),
                    chrome.page_url,
                );
                format!("/action/cross-edit?do=edit&coid={}&_={token}", c.coid)
            } else {
                String::new()
            };
            json!({
                "coid": c.coid,
                "author": c.author,
                "author_url": c.url,
                "avatar_url": comment_avatar_url(chrome, c),
                "agent": short_agent(&c.agent),
                "date": text::format_php_date(chrome.site.get_or("commentDateFormat", "Y-m-d H:i"), c.created, chrome.site.timezone()),
                "iso": text::iso8601(c.created, chrome.site.timezone()),
                "html": cross_body_html(&c.text, markdown, logged),
                "stars": stars.get(&c.coid).copied().unwrap_or(0),
                "edit_url": edit_url,
            })
        })
        .collect();
    json!(items)
}

pub fn render_single(
    env: &Environment<'_>,
    chrome: &Chrome<'_>,
    view: &SingleView<'_>,
) -> Result<String, Error> {
    let post = view.post;
    let site = chrome.site;
    let permalink = links::content_path(site, post);
    let heading = if view.locked {
        "此内容被密码保护".to_string()
    } else {
        post.title.clone()
    };
    let page_title = format_title(&heading, chrome, 1);
    let desc = if view.locked {
        "请输入密码访问".to_string()
    } else {
        text::excerpt(&post.text, 140)
    };
    let show_cloud = false;
    let post_tags: Vec<Value> = post
        .tags
        .iter()
        .map(|t| json!({"name": t.name, "url": links::tag_path(site, t)}))
        .collect();
    let mut ctx = shell(chrome, &page_title, &desc, show_cloud, &post_tags);
    let cats: Vec<Value> = post
        .categories
        .iter()
        .map(|c| json!({"name": c.name, "url": links::category_path(site, c)}))
        .collect();
    ctx["heading"] = json!(heading);
    ctx["is_page"] = json!(post.kind == "page");
    ctx["is_attachment"] = json!(post.kind == "attachment");
    ctx["author"] = json!(post.display_author());
    ctx["author_url"] = json!(links::author_path(site, post.author_id));
    ctx["date"] = json!(text::format_php_date(
        site.get_or("postDateFormat", "Y-m-d"),
        post.created,
        site.timezone()
    ));
    ctx["words"] = json!(if view.locked {
        0
    } else {
        text::word_count(&post.text)
    });
    ctx["cats"] = json!(cats);
    ctx["thumb"] = json!(if view.locked {
        Value::Null
    } else {
        json!(text::first_image(&post.text, post.field_str("thumb")))
    });
    ctx["locked"] = json!(view.locked);
    ctx["password_error"] = json!(view.password_error);
    ctx["permalink"] = json!(permalink);
    ctx["cid"] = json!(post.cid);
    ctx["datetime"] = json!(text::iso8601(post.created, site.timezone()));
    ctx["categories"] = json!(meta_links(site, &post.categories));
    ctx["tags"] = json!(tag_links(site, post));
    ctx["comment_total"] = json!(post.comments_num);
    ctx["views"] = json!(post.views.max(0));
    ctx["stars"] = json!(view.stars.max(0));
    ctx["cross_comments"] = view.cross_json.clone().unwrap_or(Value::Null);
    let html_body = if view.locked {
        String::new()
    } else if post.kind == "attachment" {
        view.attachment_html.to_string()
    } else {
        crate::plugins::filter_content(site, &text::render_post_html(&post.text))
    };
    ctx["has_math"] = json!(html_body.contains("$$") || html_body.contains("\\(") || html_body.contains("\\["));
    ctx["respond_id"] = json!(format!(
        "respond-{}-{}",
        if post.kind == "page" { "page" } else { "post" },
        post.cid
    ));
    if post.kind == "page" {
        ctx["nav_slug"] = json!(post.slug);
    }
    ctx["html"] = json!(html_body);
    ctx["newer_url"] = json!(view.newer.map(|p| links::content_path(site, p)));
    ctx["newer_title"] = json!(view.newer.map(|p| p.title.clone()));
    ctx["older_url"] = json!(view.older.map(|p| links::content_path(site, p)));
    ctx["older_title"] = json!(view.older.map(|p| p.title.clone()));
    ctx["comment_count"] = json!(view.comments.len());
    ctx["comment_html"] = json!(comment_tree_html(chrome, post, view.comments));
    ctx["comment_nav"] = json!(view.comment_nav);
    ctx["ping_html"] = json!(view.ping_html);
    ctx["allow_comment"] = json!(post.allow_comment && !view.locked && comments_open(chrome, post));
    ctx["comment_url"] = json!(links::comment_path(&permalink));
    ctx["comment_error"] = json!(view.comment_error);
    ctx["require_mail"] = json!(site.flag("commentsRequireMail", true));
    ctx["require_url"] = json!(site.flag("commentsRequireURL", false));
    ctx["remember_author"] = json!(view.remember_author);
    ctx["remember_mail"] = json!(view.remember_mail);
    ctx["remember_url"] = json!(view.remember_url);
    ctx["remember_text"] = json!(view.remember_text);
    ctx["reply_to"] = json!(view.reply_to);
    env.get_template("single.html")?.render(ctx)
}

pub fn render_404(env: &Environment<'_>, chrome: &Chrome<'_>) -> Result<String, Error> {
    let ctx = shell(
        chrome,
        &format_title("页面没找到", chrome, 1),
        chrome.site.get("description"),
        false,
        &[],
    );
    env.get_template("404.html")?.render(ctx)
}

fn theme_asset_prefix(chrome: &Chrome<'_>) -> String {
    if chrome.theme_ready {
        format!("/usr/themes/{}/assets", chrome.site.theme_name())
    } else {
        String::new()
    }
}

fn post_card(chrome: &Chrome<'_>, post: &Post) -> Value {
    let site = chrome.site;
    let locked = post.has_password() && !cookie_unlocks(chrome, post);
    let title = if locked {
        "此内容被密码保护".to_string()
    } else {
        post.title.clone()
    };
    let summary = if locked {
        "请输入密码访问".to_string()
    } else if let Some(custom) = post.field_str("customSummary") {
        custom.to_string()
    } else {
        text::excerpt(&post.text, 140)
    };
    let thumb = if locked {
        None
    } else {
        text::first_image(&post.text, post.field_str("thumb"))
    };
    let url = links::content_path(site, post);
    let content = if locked {
        "<p>请输入密码访问</p>".to_string()
    } else if let Some(custom) = post.field_str("customSummary").filter(|s| !s.is_empty()) {
        format!("<p>{}</p>", html_escape(custom))
    } else {
        text::list_html(&post.text, &url, &post.title)
    };
    json!({
        "title": title,
        "url": url,
        "summary": summary,
        "content": content,
        "categories": meta_links(site, &post.categories),
        "thumb": thumb,
        "views": post.views.max(0),
        "author": post.display_author(),
        "author_url": links::author_path(site, post.author_id),
        "date": text::format_php_date(site.get_or("postDateFormat", "Y-m-d"), post.created, site.timezone()),
        "datetime": text::iso8601(post.created, site.timezone()),
        "comment_label": comments_num_label(post.comments_num),
    })
}

fn comments_num_label(n: i64) -> String {
    match n {
        0 => "评论".into(),
        1 => "1 条评论".into(),
        n => format!("{n} 条评论"),
    }
}

fn meta_links(site: &Site, metas: &[crate::model::Meta]) -> String {
    metas
        .iter()
        .map(|m| {
            format!(
                "<a href=\"{}\">{}</a>",
                html_escape(&links::category_path(site, m)),
                html_escape(&m.name)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn tag_links(site: &Site, post: &Post) -> String {
    if post.tags.is_empty() {
        return String::new();
    }
    post.tags
        .iter()
        .map(|t| {
            format!(
                "<a href=\"{}\">{}</a>",
                html_escape(&links::tag_path(site, t)),
                html_escape(&t.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn comments_open(chrome: &Chrome<'_>, post: &Post) -> bool {
    let site = chrome.site;
    if site.flag("commentsAutoClose", false) {
        let timeout = site.int("commentsPostTimeout", 0);
        if timeout > 0 && chrome.now - post.created > timeout {
            return false;
        }
    }
    true
}

fn cookie_unlocks(chrome: &Chrome<'_>, post: &Post) -> bool {
    let root = crate::auth::site_root(
        &chrome.cfg.site_url,
        &links::site_base(&chrome.cfg.site_url, chrome.site),
    );
    let key = crate::auth::cookie_name(&root, &format!("protectPassword_{}", post.cid));
    chrome
        .cookies
        .get(&key)
        .is_some_and(|v| v == &post.password)
}

fn comment_tree_html(chrome: &Chrome<'_>, post: &Post, comments: &[Comment]) -> String {
    let markdown = chrome.site.flag("commentsMarkdown", false);
    let threaded = chrome.site.flag("commentsThreaded", true);
    let desc = chrome.site.get("commentsOrder").eq_ignore_ascii_case("DESC");
    let mut children: HashMap<i64, Vec<&Comment>> = HashMap::new();
    let mut roots: Vec<&Comment> = Vec::new();
    let ids: HashMap<i64, &Comment> = comments.iter().map(|c| (c.coid, c)).collect();
    for c in comments {
        if threaded && c.parent != 0 && ids.contains_key(&c.parent) {
            children.entry(c.parent).or_default().push(c);
        } else {
            roots.push(c);
        }
    }
    if desc {
        roots.reverse();
        for list in children.values_mut() {
            list.reverse();
        }
    }
    let mut html = String::new();
    for (i, c) in roots.iter().enumerate() {
        push_comment(&mut html, chrome, post, c, &children, 0, i + 1, markdown);
    }
    html
}

fn push_comment(
    html: &mut String,
    chrome: &Chrome<'_>,
    post: &Post,
    c: &Comment,
    children: &HashMap<i64, Vec<&Comment>>,
    depth: usize,
    sequence: usize,
    markdown: bool,
) {
    let mut class = String::from("comment-body");
    if depth > 0 {
        class.push_str(" comment-child");
        if depth % 2 == 1 {
            class.push_str(" comment-level-odd");
        } else {
            class.push_str(" comment-level-even");
        }
    } else {
        class.push_str(" comment-parent");
    }
    if sequence % 2 == 1 {
        class.push_str(" comment-odd");
    } else {
        class.push_str(" comment-even");
    }
    if c.author_id != 0 && c.author_id == post.author_id {
        class.push_str(" comment-by-author");
    }
    let cite = if c.url.is_empty() || !chrome.site.flag("commentsShowUrl", true) {
        html_escape(&c.author)
    } else {
        let rel = if chrome.site.flag("commentsUrlNofollow", true) {
            " rel=\"external nofollow\""
        } else {
            ""
        };
        format!(
            "<a href=\"{}\" target=\"_blank\"{rel}>{}</a>",
            html_escape(&c.url),
            html_escape(&c.author)
        )
    };
    let avatar = {
        let url = comment_avatar_url(chrome, c);
        if url.is_empty() {
            String::new()
        } else {
            format!(
                "<a class=\"pull-left thumb-sm comment-avatar\" rel=\"nofollow\">\
                 <img class=\"img-40px photo\" alt=\"{}\" src=\"{}\" width=\"40\" height=\"40\" loading=\"lazy\"></a>",
                html_escape(&c.author),
                html_escape(&url),
            )
        }
    };
    let when = text::format_php_date(
        chrome.site.get_or("commentDateFormat", "Y-m-d H:i"),
        c.created,
        chrome.site.timezone(),
    );
    let permalink = links::content_path(chrome.site, post);
    let respond = format!(
        "respond-{}-{}",
        if post.kind == "page" { "page" } else { "post" },
        post.cid
    );
    let max_depth = chrome.site.int("commentsMaxNestingLevels", 0);
    let can_reply = chrome.site.flag("commentsThreaded", true)
        && post.allow_comment
        && comments_open(chrome, post)
        && (max_depth <= 0 || (depth + 1) < max_depth as usize);
    let reply = if can_reply {
        format!(
            "<div class=\"comment-reply\"><a href=\"{permalink}?replyTo={id}#{respond}\" rel=\"nofollow\" onclick=\"return TypechoComment.reply('comment-{id}', {id});\">回复</a></div>",
            id = c.coid
        )
    } else {
        String::new()
    };
    html.push_str(&format!(
        "<li itemscope itemtype=\"http://schema.org/UserComments\" id=\"comment-{id}\" class=\"{class}\">\
         <div id=\"div-comment-{id}\" class=\"comment-body\">{avatar}\
         <div class=\"m-b m-l-xxl\">\
         <div class=\"comment-meta\"><span class=\"comment-author vcard\"><b class=\"fn\">{cite}</b>\
         <a data-coid=\"{id}\" class=\"post-comment-star text-muted star_talk\"><i data-feather=\"heart\"></i>&nbsp;<span class=\"star_count\"></span></a></span>\
         <div class=\"comment-metadata\"><a href=\"{permalink}#comment-{id}\">\
         <time class=\"format_time\" datetime=\"{iso}\">{when}</time></a></div></div>\
         <div class=\"comment-content\"><div class=\"comment-content-true\" itemprop=\"commentText\">{body}</div></div>\
         {reply}</div></div>",
        id = c.coid,
        iso = html_escape(&text::iso8601(c.created, chrome.site.timezone())),
        when = html_escape(&when),
        body = text::render_comment_html(&c.text, markdown),
    ));
    if let Some(list) = children.get(&c.coid) {
        html.push_str("<div class=\"comment-children\" itemprop=\"discusses\"><ol class=\"comment-list\">");
        for (i, ch) in list.iter().enumerate() {
            push_comment(html, chrome, post, ch, children, depth + 1, i + 1, markdown);
        }
        html.push_str("</ol></div>");
    }
    html.push_str("</li>");
}

/// UA 简写（说说 footer「发自」用；无则空，模板省略）。
fn short_agent(agent: &str) -> String {
    let a = agent.trim();
    if a.is_empty() {
        return String::new();
    }
    text::truncate_chars(&a.replace(['\r', '\n', '\t'], " "), 24)
}

/// 说说正文：`[secret]…[/secret]` 仅登录可见，否则占位。
fn cross_body_html(raw: &str, markdown: bool, logged: bool) -> String {
    let (start, end) = ("[secret]", "[/secret]");
    if let (Some(i), Some(j)) = (raw.find(start), raw.rfind(end)) {
        let inner = &raw[i + start.len()..j];
        if logged {
            return text::render_comment_html(inner, markdown);
        }
        return "<p>此条为私密说说，仅发布者可见</p>".to_string();
    }
    text::render_comment_html(raw, markdown)
}

/// 评论头像（gravatar，同 Typecho 口径；关闭头像或无邮箱时为空）。
fn comment_avatar_url(chrome: &Chrome<'_>, c: &Comment) -> String {
    if !chrome.site.flag("commentsAvatar", true) || c.mail.is_empty() {
        return String::new();
    }
    let hash = gravatar_hash(&c.mail);
    let prefix = chrome.cfg.gravatar_prefix.trim_end_matches('/');
    let rating = chrome.site.get_or("commentsAvatarRating", "G");
    format!("{prefix}/{hash}?s=40&r={rating}&d=mm")
}

fn gravatar_hash(mail: &str) -> String {
    use md5::{Digest, Md5};
    hex::encode(Md5::digest(mail.trim().to_lowercase().as_bytes()))
}

fn public_logout(chrome: &Chrome<'_>) -> String {
    let token = crate::auth::security_token(
        chrome.site.get("secret"),
        Some((chrome.viewer_auth, chrome.viewer_uid)),
        chrome.page_url,
    );
    links::with_token(
        &links::action_url(chrome.site, "/action/logout", false),
        &token,
    )
}

fn shell(
    chrome: &Chrome<'_>,
    page_title: &str,
    description: &str,
    _show_tag_cloud: bool,
    _post_tags: &[Value],
) -> Value {
    let site = chrome.site;
    let base = links::site_base(&chrome.cfg.site_url, site);
    let desc = if description.is_empty() {
        site.get("description").to_string()
    } else {
        description.to_string()
    };
    let static_path = theme_asset_prefix(chrome);
    let hot_posts: Vec<Value> = chrome.hot_posts.iter().map(|p| {
        let title = if p.has_password() { "密码保护的文章".to_string() } else { p.title.clone() };
        json!({
            "title": title,
            "url": links::content_path(site, p),
            "views": p.views.max(0),
            "date": text::format_php_date(site.get_or("postDateFormat", "Y-m-d"), p.created, site.timezone()),
            "thumb": text::first_image(&p.text, p.field_str("thumb")),
        })
    }).collect();
    let random_posts: Vec<Value> = chrome.random_posts.iter().map(|p| {
        let title = if p.has_password() { "密码保护的文章".to_string() } else { p.title.clone() };
        json!({
            "title": title,
            "url": links::content_path(site, p),
            "views": p.views.max(0),
            "date": text::format_php_date(site.get_or("postDateFormat", "Y-m-d"), p.created, site.timezone()),
            "thumb": text::first_image(&p.text, p.field_str("thumb")),
        })
    }).collect();
    let friend_links: Vec<Value> = chrome.friend_links.iter().map(|l| json!({
        "name": l.name, "url": l.url, "description": l.description, "image": l.image,
    })).collect();
    let categories: Vec<Value> = site.categories.iter().map(|c| json!({
        "name": c.name, "url": links::category_path(site, c), "count": c.count,
    })).collect();
    let archives_list: Vec<Value> = chrome.archives.iter().map(|(year, month, ts)| {
        let mut vals = HashMap::new();
        vals.insert("year".into(), format!("{year:04}"));
        vals.insert("month".into(), format!("{month:02}"));
        let href = links::route_url(site, "archive_month", &vals)
            .unwrap_or_else(|| links::with_index(site, &format!("/{year:04}/{month:02}/")));
        json!({"year": year, "month": month, "url": href,
               "label": text::format_php_date("F Y", *ts, site.timezone())})
    }).collect();
    let recent_posts: Vec<Value> = chrome.recent_posts.iter().map(|p| {
        let title = if p.has_password() { "密码保护的文章".to_string() } else { p.title.clone() };
        json!({"title": title, "url": links::content_path(site, p)})
    }).collect();
    let recent_comments: Vec<Value> = chrome.recent.iter().map(|(c, p)| {
        let href = format!("{}#comment-{}", links::content_path(site, p), c.coid);
        json!({"author": c.author, "url": href,
               "excerpt": text::truncate_chars(&text::plain_text(&c.text), 35)})
    }).collect();
    json!({
        "page_title": page_title,
        "site_title": site.get_or("title", "Tiebidli"),
        "site_description": site.get("description"),
        "description": desc,
        "keywords": site.get("keywords"),
        "base": base,
        "static": static_path,
        "theme_ready": chrome.theme_ready,
        "logo_url": site.theme_str("logoUrl"),
        "year": text::ymd(chrome.now, site.timezone()).0,
        "feed_url": links::feed_path(site, "/"),
        "comments_feed_url": links::feed_path(site, "/comments/"),
        "owner_name": owner_name(chrome),
        "feed_abs": abs(&base, &links::feed_path(site, "/")),
        "atom_url": links::feed_path(site, "/atom/"),
        "atom_abs": abs(&base, &links::feed_path(site, "/atom/")),
        "pages": site.pages.iter().map(|p| json!({
            "title": p.title,
            "slug": p.slug,
            "url": links::content_path(site, p),
        })).collect::<Vec<_>>(),
        "nav_labels": nav_labels(chrome.accept_language),
        "nav_index": false,
        "nav_slug": "",
        "viewer_name": chrome.viewer_name,
        "admin_url": "/admin/index.php",
        "logout_url": if chrome.viewer_name.is_some() { public_logout(chrome) } else { String::new() },
        "sidebar": sidebar_html(chrome),
        "hot_posts": hot_posts,
        "random_posts": random_posts,
        "friend_links": friend_links,
        "categories": categories,
        "archives_list": archives_list,
        "recent_posts": recent_posts,
        "recent_comments": recent_comments,
        "all_tags": site.tags.iter().map(|t| json!({
            "name": t.name, "url": links::tag_path(site, t), "count": t.count,
        })).collect::<Vec<_>>(),
        "site_stats": site_stats_json(chrome),
        "calendar_json": serde_json::to_string(&archives_list).unwrap_or_else(|_| "[]".into()),
        "cats_json": serde_json::to_string(&categories).unwrap_or_else(|_| "[]".into()),
        "base_json": serde_json::to_string(&base).unwrap_or_else(|_| "\"\"".into()),
        "show_archive_title": false,
        "plugin_header": crate::plugins::public_header(site),
        "plugin_footer": crate::plugins::public_footer_page(site, chrome.access_ids.0, chrome.access_ids.1),
    })
}

/// 侧栏分组文案（对齐现网 i18n 实测）：
/// zh（zh-CN / zh-HK / zh-TW …）→ 首页/分类/页面/友链；其余 → Home/Categories/Pages/Links。
/// 优先级：Accept-Language 主语言；无头或解析不出 → 英文（与现网 en 实测一致）。
/// 自定义页面标题（时间线、nCoV 等）走数据源原标题，不在这里硬翻。
pub fn nav_labels(accept_language: Option<&str>) -> Value {
    let zh = accept_language
        .and_then(|h| h.split(',').next())
        .map(|tag| tag.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .is_some_and(|t| t == "zh" || t.starts_with("zh-"));
    if zh {
        json!({"lang": "zh-CN", "home": "首页", "categories": "分类", "pages": "页面", "links": "友链"})
    } else {
        json!({"lang": "en", "home": "Home", "categories": "Categories", "pages": "Pages", "links": "Links"})
    }
}

fn sidebar_on(site: &Site, key: &str) -> bool {
    match site.theme.get_key("sidebarBlock") {
        None => true,
        Some(value) => value.string_list().iter().any(|item| item == key),
    }
}

/// 站点统计（Blog Info）：文章数 / 评论数 / 运行天数 / 最后活动。
fn site_stats_json(chrome: &Chrome<'_>) -> Value {
    let days = if chrome.stats.first_created > 0 && chrome.now >= chrome.stats.first_created {
        (chrome.now - chrome.stats.first_created) / 86400 + 1
    } else {
        0
    };
    json!({
        "posts": chrome.stats.posts,
        "comments": chrome.stats.comments,
        "days": days,
        "days_label": days_label(days),
        "last_activity": chrome.stats.last_activity,
        "last_activity_label": rel_time(chrome.now, chrome.stats.last_activity),
    })
}

/// 站长名：取最新文章作者（与现网 ModerRAS 一致），无文章回站点标题。
fn owner_name(chrome: &Chrome<'_>) -> String {
    chrome
        .recent_posts
        .first()
        .map(|p| p.display_author())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| chrome.site.get_or("title", "Tiebidli").to_string())
}

/// 运行天数：满年按现网 `9 Y 156 D` 格式，否则 `N D`。
fn days_label(days: i64) -> String {
    if days >= 365 {
        format!("{} Y {} D", days / 365, days % 365)
    } else {
        format!("{days}")
    }
}

/// 最后活动：贴近现网 `1 Year Ago` 口径的相对时间。
fn rel_time(now: i64, ts: i64) -> String {
    let d = now.saturating_sub(ts);
    if ts <= 0 {
        return "-".into();
    }
    if d < 3600 {
        format!("{} Min Ago", (d / 60).max(1))
    } else if d < 86400 {
        format!("{} H Ago", d / 3600)
    } else if d < 86400 * 30 {
        format!("{} D Ago", d / 86400)
    } else if d < 86400 * 365 {
        format!("{} Mon Ago", d / (86400 * 30))
    } else {
        format!("{} Year Ago", d / (86400 * 365))
    }
}

fn sidebar_html(chrome: &Chrome<'_>) -> String {
    let site = chrome.site;
    let mut html = String::new();
    if sidebar_on(site, "ShowRecentPosts") {
        html.push_str("<section class=\"widget\"><h3 class=\"widget-title\">最新文章</h3><ul class=\"widget-list\">");
        for post in chrome.recent_posts {
            let title = if post.has_password() {
                "密码保护的文章"
            } else {
                post.title.as_str()
            };
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                html_escape(&links::content_path(site, post)),
                html_escape(title)
            ));
        }
        html.push_str("</ul></section>");
    }
    if !chrome.hot_posts.is_empty() {
        html.push_str("<section class=\"widget widget-hots\"><h3 class=\"widget-title\">热门文章</h3><ul class=\"widget-list\">");
        for post in chrome.hot_posts.iter().take(10) {
            let title = if post.has_password() {
                "密码保护的文章"
            } else {
                post.title.as_str()
            };
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a> <span class=\"views\">浏览次数: {}</span></li>",
                html_escape(&links::content_path(site, post)),
                html_escape(title),
                post.views.max(0),
            ));
        }
        html.push_str("</ul></section>");
    }
    if sidebar_on(site, "ShowRecentComments") {
        html.push_str("<section class=\"widget\"><h3 class=\"widget-title\">最近回复</h3><ul class=\"widget-list\">");
        for (comment, post) in chrome.recent {
            let href = format!("{}#comment-{}", links::content_path(site, post), comment.coid);
            let excerpt = text::truncate_chars(&text::plain_text(&comment.text), 35);
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a>: {}</li>",
                html_escape(&href),
                html_escape(&comment.author),
                html_escape(&excerpt)
            ));
        }
        html.push_str("</ul></section>");
    }
    if sidebar_on(site, "ShowCategory") {
        html.push_str("<section class=\"widget\"><h3 class=\"widget-title\">分类</h3>");
        html.push_str(&category_widget(site, 0));
        html.push_str("</section>");
    }
    if sidebar_on(site, "ShowArchive") {
        html.push_str("<section class=\"widget\"><h3 class=\"widget-title\">归档</h3><ul class=\"widget-list\">");
        for (year, month, ts) in chrome.archives {
            let mut vals = HashMap::new();
            vals.insert("year".into(), format!("{year:04}"));
            vals.insert("month".into(), format!("{month:02}"));
            let href = links::route_url(site, "archive_month", &vals)
                .unwrap_or_else(|| links::with_index(site, &format!("/{year:04}/{month:02}/")));
            let label = text::format_php_date("F Y", *ts, site.timezone());
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                html_escape(&href),
                html_escape(&label)
            ));
        }
        html.push_str("</ul></section>");
    }
    if !chrome.friend_links.is_empty() {
        html.push_str("<section class=\"widget widget-links\"><h3 class=\"widget-title\">友情链接</h3><ul class=\"widget-list\">");
        for link in chrome.friend_links {
            html.push_str(&format!(
                "<li><a href=\"{}\" title=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a></li>",
                html_escape(&link.url),
                html_escape(&link.description),
                html_escape(&link.name),
            ));
        }
        html.push_str("</ul></section>");
    }
    if sidebar_on(site, "ShowOther") {
        html.push_str("<section class=\"widget\"><h3 class=\"widget-title\">其它</h3><ul class=\"widget-list\">");
        if let Some(name) = chrome.viewer_name {
            let logout = public_logout(chrome);
            html.push_str(&format!(
                "<li class=\"last\"><a href=\"/admin/index.php\">进入后台 ({name})</a></li><li><a href=\"{logout}\">退出</a></li>",
                name = html_escape(name),
                logout = html_escape(&logout)
            ));
        } else {
            html.push_str("<li class=\"last\"><a href=\"/admin/login.php\">登录</a></li>");
        }
        html.push_str(&format!(
            "<li><a href=\"{}\">文章 RSS</a></li><li><a href=\"{}\">评论 RSS</a></li><li><a href=\"https://typecho.org\">Typecho</a></li></ul></section>",
            html_escape(&links::feed_path(site, "/")),
            html_escape(&links::feed_path(site, "/comments/"))
        ));
    }
    html
}

fn category_widget(site: &Site, parent: i64) -> String {
    let rows: Vec<_> = site
        .categories
        .iter()
        .filter(|c| c.parent == parent)
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    let mut html = String::from("<ul class=\"widget-list\">");
    for c in rows {
        let level = category_level(site, c.parent);
        let mut class = format!("category-level-{level}");
        if level > 0 {
            class.push_str(" category-child");
            if level % 2 == 1 {
                class.push_str(" category-level-odd");
            } else {
                class.push_str(" category-level-even");
            }
        } else {
            class.push_str(" category-parent");
        }
        html.push_str(&format!(
            "<li class=\"{class}\"><a href=\"{}\">{}</a>",
            html_escape(&links::category_path(site, c)),
            html_escape(&c.name)
        ));
        html.push_str(&category_widget(site, c.mid));
        html.push_str("</li>");
    }
    html.push_str("</ul>");
    html
}

fn category_level(site: &Site, mut parent: i64) -> i64 {
    let mut level = 0;
    let mut guard = 0;
    while parent != 0 && guard < 16 {
        level += 1;
        parent = site
            .categories
            .iter()
            .find(|c| c.mid == parent)
            .map(|c| c.parent)
            .unwrap_or(0);
        guard += 1;
    }
    level
}

fn format_title(heading: &str, chrome: &Chrome<'_>, page: i64) -> String {
    let site_title = chrome.site.get_or("title", "Tiebidli");
    let prefix = if page > 1 {
        format!("第{page}页 - ")
    } else {
        String::new()
    };
    if heading == site_title {
        format!("{prefix}{site_title}")
    } else {
        format!("{prefix}{heading} - {site_title}")
    }
}

#[cfg(test)]
mod tests {
    use super::nav_labels;

    fn label(header: Option<&str>, key: &str) -> String {
        nav_labels(header)[key].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn nav_labels_default_english() {
        assert_eq!(label(None, "home"), "Home");
        assert_eq!(label(Some(""), "categories"), "Categories");
        assert_eq!(label(Some("en-US,en;q=0.9"), "pages"), "Pages");
        assert_eq!(label(Some("fr-FR,fr;q=0.8"), "links"), "Links");
    }

    #[test]
    fn nav_labels_chinese() {
        for h in ["zh-CN,zh;q=0.9", "zh", "zh-HK", "ZH-tw;q=0.8", " zh-CN "] {
            assert_eq!(label(Some(h), "home"), "首页", "{h}");
            assert_eq!(label(Some(h), "categories"), "分类", "{h}");
            assert_eq!(label(Some(h), "pages"), "页面", "{h}");
            assert_eq!(label(Some(h), "links"), "友链", "{h}");
        }
        assert_eq!(nav_labels(Some("zh-CN"))["lang"], "zh-CN");
        assert_eq!(nav_labels(None)["lang"], "en");
    }
}
