//! Typecho 1.2 的公开 XML-RPC。入口是 `/action/xmlrpc`。
//! `allowXmlRpc`：0 关闭，1 不开 pingback，2 含 pingback。缺省按安装默认值 2。

use crate::auth::{self, SessionUser};
use crate::config::Config;
use crate::db::Db;
use crate::links;
use crate::model::{Post, Site};
use crate::repo::NewComment;
use crate::text::{self, html_escape};
use crate::write::{self, ContentInput, MetaInput};
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use std::path::PathBuf;
use std::time::Duration;

const METHODS: &[&str] = &[
    "system.listMethods",
    "metaWeblog.newPost",
    "metaWeblog.editPost",
    "metaWeblog.getPost",
    "metaWeblog.getRecentPosts",
    "metaWeblog.getCategories",
    "metaWeblog.newMediaObject",
    "metaWeblog.deletePost",
    "blogger.getUsersBlogs",
    "blogger.getUserInfo",
    "blogger.getRecentPosts",
    "blogger.deletePost",
    "wp.getUsersBlogs",
    "wp.newCategory",
    "wp.getTags",
    "wp.getPages",
    "wp.newPage",
    "wp.editPage",
    "wp.deletePage",
    "mt.getCategoryList",
    "mt.getRecentPostTitles",
    "mt.publishPost",
    "pingback.ping",
];

pub async fn handle(
    db: &Db,
    cfg: &Config,
    site: &Site,
    method: &Method,
    query: &std::collections::HashMap<String, String>,
    body: &[u8],
    now: i64,
    headers: &HeaderMap,
) -> anyhow::Result<Response> {
    let allow = site.int("allowXmlRpc", 2);
    if allow <= 0 {
        return Ok(text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "请求的地址不存在"));
    }
    let base = links::site_base(&cfg.site_url, site);
    let endpoint = links::abs(&base, &links::action_url(site, "/action/xmlrpc", false));
    if *method == Method::GET && query.contains_key("rsd") {
        return Ok(text_response(StatusCode::OK, "text/xml; charset=utf-8", &rsd(&base, &endpoint)));
    }
    if *method == Method::GET && query.contains_key("wlw") {
        return Ok(text_response(StatusCode::OK, "text/xml; charset=utf-8", wlw()));
    }
    if *method != Method::POST {
        return Ok(xml_body(&fault(405, "需要 POST")));
    }
    let text = String::from_utf8_lossy(body);
    let Some((name, params)) = parse_call(&text) else {
        return Ok(xml_body(&fault(400, "无法解析 XML-RPC")));
    };
    if allow < 2 && name == "pingback.ping" {
        return Ok(xml_body(&fault(404, "方法不存在")));
    }
    let xml = dispatch(db, cfg, site, &base, &name, &params, now, headers).await?;
    Ok(xml_body(&xml))
}

macro_rules! gate {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(xml) => return Ok(xml),
        }
    };
}

async fn dispatch(
    db: &Db,
    cfg: &Config,
    site: &Site,
    base: &str,
    name: &str,
    params: &[XVal],
    now: i64,
    headers: &HeaderMap,
) -> anyhow::Result<String> {
    match name {
        "system.listMethods" => Ok(ok_array(
            &METHODS
                .iter()
                .map(|m| val_string(m))
                .collect::<Vec<_>>(),
        )),
        "blogger.getUsersBlogs" | "metaWeblog.getUsersBlogs" | "wp.getUsersBlogs" => {
            let user = gate!(auth_user(db, params, 1).await);
            Ok(ok_array(&[blog_struct(site, base, &user)]))
        }
        "blogger.getUserInfo" => {
            let user = gate!(auth_user(db, params, 1).await);
            Ok(ok_value(&val_struct(&[
                ("userid", val_string(&user.uid.to_string())),
                ("nickname", val_string(&user.label())),
                ("email", val_string(&user.mail)),
                ("url", val_string(&user.url)),
            ])))
        }
        "metaWeblog.newPost" => {
            let user = gate!(auth_user(db, params, 1).await);
            let content = params.get(3).cloned().unwrap_or(XVal::Struct(vec![]));
            let publish = params.get(4).map(|v| v.as_bool()).unwrap_or(true);
            let cid = gate!(save_post(db, site, &user, None, &content, publish, "post", now).await);
            Ok(ok_value(&val_int(cid)))
        }
        "metaWeblog.editPost" => {
            let user = gate!(auth_user(db, params, 1).await);
            let cid = params.first().map(|v| v.as_int()).unwrap_or(0);
            let content = params.get(3).cloned().unwrap_or(XVal::Struct(vec![]));
            let publish = params.get(4).map(|v| v.as_bool()).unwrap_or(true);
            let cid = gate!(save_post(db, site, &user, Some(cid), &content, publish, "post", now).await);
            Ok(ok_value(&val_bool(cid > 0)))
        }
        "wp.newPage" => {
            let user = gate!(auth_user(db, params, 1).await);
            gate!(require_editor(&user));
            let content = params.get(3).cloned().unwrap_or(XVal::Struct(vec![]));
            let publish = params.get(4).map(|v| v.as_bool()).unwrap_or(true);
            let cid = gate!(save_post(db, site, &user, None, &content, publish, "page", now).await);
            Ok(ok_value(&val_int(cid)))
        }
        "wp.editPage" => {
            let user = gate!(auth_user(db, params, 2).await);
            gate!(require_editor(&user));
            let cid = params.get(1).map(|v| v.as_int()).unwrap_or(0);
            let content = params.get(4).cloned().unwrap_or(XVal::Struct(vec![]));
            let publish = params.get(5).map(|v| v.as_bool()).unwrap_or(true);
            let cid = gate!(save_post(db, site, &user, Some(cid), &content, publish, "page", now).await);
            Ok(ok_value(&val_bool(cid > 0)))
        }
        "metaWeblog.getPost" => {
            let user = gate!(auth_user(db, params, 1).await);
            let cid = params.first().map(|v| v.as_int()).unwrap_or(0);
            let Some(post) = db.content_by_id(cid).await? else {
                return Ok(fault(404, "文章不存在"));
            };
            if let Err(xml) = ensure_read(&user, &post) {
                return Ok(xml);
            }
            Ok(ok_value(&post_struct(site, base, &post)))
        }
        "metaWeblog.getRecentPosts" | "blogger.getRecentPosts" => {
            let user = gate!(auth_user(db, params, 1).await);
            let limit = params.get(3).map(|v| v.as_int()).unwrap_or(10).clamp(1, 50);
            let posts = visible_posts(db, &user, "post", limit).await?;
            let items = posts
                .iter()
                .map(|p| post_struct(site, base, p))
                .collect::<Vec<_>>();
            Ok(ok_array(&items))
        }
        "wp.getPages" => {
            let user = gate!(auth_user(db, params, 1).await);
            let posts = visible_posts(db, &user, "page", 50).await?;
            let items = posts
                .iter()
                .map(|p| post_struct(site, base, p))
                .collect::<Vec<_>>();
            Ok(ok_array(&items))
        }
        "mt.getRecentPostTitles" => {
            let user = gate!(auth_user(db, params, 1).await);
            let posts = visible_posts(db, &user, "post", 20).await?;
            let items = posts
                .iter()
                .map(|p| {
                    val_struct(&[
                        ("postid", val_string(&p.cid.to_string())),
                        ("userid", val_string(&p.author_id.to_string())),
                        ("title", val_string(&p.title)),
                        ("dateCreated", val_date(p.created)),
                    ])
                })
                .collect::<Vec<_>>();
            Ok(ok_array(&items))
        }
        "metaWeblog.getCategories" | "mt.getCategoryList" => {
            let _user = gate!(auth_user(db, params, 1).await);
            let metas = db.list_metas("category").await?;
            let items = metas
                .iter()
                .map(|m| {
                    val_struct(&[
                        ("categoryId", val_string(&m.mid.to_string())),
                        ("parentId", val_string(&m.parent.to_string())),
                        ("categoryName", val_string(&m.name)),
                        ("description", val_string(&m.description)),
                        ("htmlUrl", val_string(&links::abs(base, &links::category_path(site, m)))),
                        ("rssUrl", val_string(&links::abs(base, &links::feed_of(site, "category", &links::category_vals(site, m), "/")))),
                    ])
                })
                .collect::<Vec<_>>();
            Ok(ok_array(&items))
        }
        "wp.getTags" => {
            let _user = gate!(auth_user(db, params, 1).await);
            let metas = db.list_metas("tag").await?;
            let items = metas
                .iter()
                .map(|m| {
                    val_struct(&[
                        ("tag_id", val_string(&m.mid.to_string())),
                        ("name", val_string(&m.name)),
                        ("slug", val_string(&m.slug)),
                        ("count", val_int(m.count)),
                    ])
                })
                .collect::<Vec<_>>();
            Ok(ok_array(&items))
        }
        "wp.newCategory" => {
            let user = gate!(auth_user(db, params, 1).await);
            gate!(require_editor(&user));
            let spec = params.get(3).cloned().unwrap_or(XVal::Struct(vec![]));
            let name = spec.get("name").map(|v| v.as_str()).unwrap_or_default();
            if name.trim().is_empty() {
                return Ok(fault(400, "分类名不能为空"));
            }
            let mid = db
                .save_meta(&MetaInput {
                    mid: None,
                    name,
                    slug: spec.get("slug").map(|v| v.as_str()).unwrap_or_default(),
                    kind: "category".into(),
                    description: spec
                        .get("description")
                        .map(|v| v.as_str())
                        .unwrap_or_default(),
                    parent: spec
                        .get("parent_id")
                        .or_else(|| spec.get("parent"))
                        .map(|v| v.as_int())
                        .unwrap_or(0),
                })
                .await?;
            Ok(ok_value(&val_int(mid)))
        }
        "metaWeblog.newMediaObject" | "wp.uploadFile" => {
            let user = gate!(auth_user(db, params, 1).await);
            if !user.pass("contributor") {
                return Ok(fault(403, "没有上传权限"));
            }
            let spec = params.get(3).cloned().unwrap_or(XVal::Struct(vec![]));
            let saved = gate!(save_media(db, cfg, site, &user, &spec, now).await);
            Ok(ok_value(&saved))
        }
        "mt.publishPost" => {
            let user = gate!(auth_user(db, params, 1).await);
            let cid = params.first().map(|v| v.as_int()).unwrap_or(0);
            let Some(post) = db.content_by_id(cid).await? else {
                return Ok(fault(404, "文章不存在"));
            };
            if let Err((code, msg)) = ensure_write(&user, &post) {
                return Ok(fault(code, &msg));
            }
            let status = if user.pass("editor") { "publish" } else { "waiting" };
            db.save_content(&from_post(&post, status, post.created)).await?;
            Ok(ok_value(&val_bool(true)))
        }
        "blogger.deletePost" | "metaWeblog.deletePost" | "wp.deletePage" => {
            let (user, cid) = if name == "wp.deletePage" {
                (
                    gate!(auth_user(db, params, 1).await),
                    params.get(3).map(|v| v.as_int()).unwrap_or(0),
                )
            } else {
                (
                    gate!(auth_user(db, params, 2).await),
                    params.get(1).map(|v| v.as_int()).unwrap_or(0),
                )
            };
            let Some(post) = db.content_by_id(cid).await? else {
                return Ok(fault(404, "内容不存在"));
            };
            if let Err((code, msg)) = ensure_write(&user, &post) {
                return Ok(fault(code, &msg));
            }
            db.delete_content(cid).await?;
            Ok(ok_value(&val_bool(true)))
        }
        "pingback.ping" => {
            let source = params.first().map(|v| v.as_str()).unwrap_or_default();
            let target = params.get(1).map(|v| v.as_str()).unwrap_or_default();
            match pingback(db, site, &source, &target, now, headers).await? {
                Ok(id) => Ok(ok_value(&val_int(id))),
                Err((code, msg)) => Ok(fault(code, &msg)),
            }
        }
        _ => Ok(fault(404, "方法不存在")),
    }
}

async fn save_post(
    db: &Db,
    site: &Site,
    user: &SessionUser,
    cid: Option<i64>,
    content: &XVal,
    publish: bool,
    kind: &str,
    now: i64,
) -> Result<i64, String> {
    if !user.pass("contributor") {
        return Err(fault(403, "没有写权限"));
    }
    let existing = if let Some(cid) = cid {
        match db.content_by_id(cid).await.map_err(|e| fault(500, &e.to_string()))? {
            Some(post) => {
                if let Err((code, msg)) = ensure_write(user, &post) {
                    return Err(fault(code, &msg));
                }
                Some(post)
            }
            None => return Err(fault(404, "内容不存在")),
        }
    } else {
        None
    };
    let title = content
        .get("title")
        .map(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "未命名文档".into());
    let mut text = content
        .get("description")
        .map(|v| v.as_str())
        .unwrap_or_default();
    if let Some(more) = content.get("mt_text_more").map(|v| v.as_str()) {
        if !more.is_empty() {
            text = format!("{text}\n<!--more-->\n{more}");
        }
    }
    if site.flag("xmlrpcMarkdown", false) && !text.contains("<!--markdown-->") {
        text = format!("<!--markdown-->{text}");
    }
    let slug = content
        .get("wp_slug")
        .or_else(|| content.get("slug"))
        .or_else(|| content.get("mt_basename"))
        .map(|v| v.as_str())
        .unwrap_or_default();
    let mut status = if publish {
        "publish".to_string()
    } else {
        "draft".to_string()
    };
    if let Some(raw) = content
        .get(if kind == "page" { "page_status" } else { "post_status" })
        .map(|v| v.as_str())
    {
        status = match raw.as_str() {
            "publish" | "published" => "publish",
            "private" => "private",
            "pending" | "waiting" => "waiting",
            "draft" => "draft",
            other => other,
        }
        .to_string();
    }
    if !user.pass("editor") && status == "publish" {
        status = "waiting".into();
    }
    let created = content
        .get("dateCreated")
        .and_then(|v| parse_date(&v.as_str()))
        .unwrap_or_else(|| existing.as_ref().map(|p| p.created).unwrap_or(now));
    let categories = category_ids(db, content).await?;
    let tags = content
        .get("mt_keywords")
        .map(|v| v.as_str())
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let allow_comment = flag_or(
        content.get("mt_allow_comments"),
        existing.as_ref().map(|p| p.allow_comment).unwrap_or(true),
    );
    let allow_ping = flag_or(
        content.get("mt_allow_pings"),
        existing.as_ref().map(|p| p.allow_ping).unwrap_or(true),
    );
    let password = content
        .get("wp_password")
        .map(|v| v.as_str())
        .unwrap_or_else(|| existing.as_ref().map(|p| p.password.clone()).unwrap_or_default());
    let cid = db
        .save_content(&ContentInput {
            cid: existing.as_ref().map(|p| p.cid),
            title,
            slug,
            text,
            created,
            kind: kind.into(),
            status: status.into(),
            password,
            allow_comment,
            allow_ping,
            allow_feed: existing.as_ref().map(|p| p.allow_feed).unwrap_or(true),
            author_id: existing.as_ref().map(|p| p.author_id).unwrap_or(user.uid),
            parent: 0,
            sort_order: content
                .get("wp_page_order")
                .map(|v| v.as_int())
                .unwrap_or_else(|| existing.as_ref().map(|p| p.sort_order).unwrap_or(0)),
            categories,
            tags,
        })
        .await
        .map_err(|e| fault(500, &e.to_string()))?;
    Ok(cid)
}

fn from_post(post: &Post, status: &str, created: i64) -> ContentInput {
    ContentInput {
        cid: Some(post.cid),
        title: post.title.clone(),
        slug: post.slug.clone(),
        text: post.text.clone(),
        created,
        kind: post.kind.clone(),
        status: status.into(),
        password: post.password.clone(),
        allow_comment: post.allow_comment,
        allow_ping: post.allow_ping,
        allow_feed: post.allow_feed,
        author_id: post.author_id,
        parent: post.parent,
        sort_order: post.sort_order,
        categories: post.categories.iter().map(|c| c.mid).collect(),
        tags: post.tags.iter().map(|t| t.name.clone()).collect(),
    }
}

async fn category_ids(db: &Db, content: &XVal) -> Result<Vec<i64>, String> {
    let Some(list) = content.get("categories").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for item in list {
        let name = item.as_str();
        if name.trim().is_empty() {
            continue;
        }
        let mid = if let Some(mid) = db
            .meta_by_name("category", &name)
            .await
            .map_err(|e| fault(500, &e.to_string()))?
        {
            mid
        } else {
            db.save_meta(&MetaInput {
                mid: None,
                name: name.clone(),
                slug: String::new(),
                kind: "category".into(),
                description: name,
                parent: 0,
            })
            .await
            .map_err(|e| fault(500, &e.to_string()))?
        };
        ids.push(mid);
    }
    Ok(ids)
}

async fn save_media(
    db: &Db,
    cfg: &Config,
    site: &Site,
    user: &SessionUser,
    spec: &XVal,
    now: i64,
) -> Result<String, String> {
    let name = spec
        .get("name")
        .map(|v| v.as_str())
        .unwrap_or_else(|| "file.bin".into());
    let mime = spec
        .get("type")
        .map(|v| v.as_str())
        .unwrap_or_else(|| "application/octet-stream".into());
    let bytes = spec
        .get("bits")
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if bytes.is_empty() {
        return Err(fault(400, "没有文件内容"));
    }
    let safe = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file.bin")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect::<String>();
    let safe = if safe.is_empty() { "file.bin".into() } else { safe };
    let ext = safe.rsplit('.').next().unwrap_or("bin").to_ascii_lowercase();
    let local = chrono::DateTime::from_timestamp(now + site.timezone(), 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(now, 0).unwrap());
    let folder = format!("{:04}/{:02}", local.format("%Y"), local.format("%m"));
    let file_name = format!("{}-{safe}", &auth::md5_hex(&bytes)[..12]);
    let root = cfg
        .uploads_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("usr/uploads"));
    let dir = root.join(&folder);
    std::fs::create_dir_all(&dir).map_err(|e| fault(500, &e.to_string()))?;
    std::fs::write(dir.join(&file_name), &bytes).map_err(|e| fault(500, &e.to_string()))?;
    let rel = format!("usr/uploads/{folder}/{file_name}");
    let text = write::attachment_text(&safe, &rel, bytes.len() as i64, &ext, &mime);
    let cid = db
        .save_content(&ContentInput {
            cid: None,
            title: safe.clone(),
            slug: String::new(),
            text,
            created: now,
            kind: "attachment".into(),
            status: "publish".into(),
            password: String::new(),
            allow_comment: false,
            allow_ping: false,
            allow_feed: false,
            author_id: user.uid,
            parent: 0,
            sort_order: 0,
            categories: Vec::new(),
            tags: Vec::new(),
        })
        .await
        .map_err(|e| fault(500, &e.to_string()))?;
    let base = links::site_base(&cfg.site_url, site);
    let url = format!("{base}/{rel}");
    let _ = cid;
    Ok(val_struct(&[
        ("url", val_string(&url)),
        ("file", val_string(&safe)),
        ("type", val_string(&mime)),
    ]))
}

async fn visible_posts(
    db: &Db,
    user: &SessionUser,
    kind: &str,
    limit: i64,
) -> anyhow::Result<Vec<Post>> {
    let posts = db.list_contents(kind, limit.max(1)).await?;
    Ok(posts
        .into_iter()
        .filter(|p| user.pass("editor") || p.author_id == user.uid)
        .collect())
}

async fn pingback(
    db: &Db,
    site: &Site,
    source: &str,
    target: &str,
    now: i64,
    headers: &HeaderMap,
) -> anyhow::Result<Result<i64, (i32, String)>> {
    let Ok(source_url) = url::Url::parse(source) else {
        return Ok(Err((16, "源地址服务器错误".into())));
    };
    if source_url.scheme() != "http" && source_url.scheme() != "https" {
        return Ok(Err((16, "源地址服务器错误".into())));
    }
    let Ok(target_url) = url::Url::parse(target) else {
        return Ok(Err((33, "这个目标地址不存在".into())));
    };
    let path = crate::http::normalize_path(target_url.path());
    let Some(post) = find_public(db, site, &path, now).await? else {
        return Ok(Err((33, "这个目标地址不存在".into())));
    };
    if !post.allow_ping {
        return Ok(Err((49, "目标地址禁止Ping".into())));
    }
    if db.ping_exists(post.cid, source).await? {
        return Ok(Err((48, "PingBack已经存在".into())));
    }
    let page = match fetch_http(source).await {
        Some(page) => page,
        None => return Ok(Err((16, "源地址服务器错误".into()))),
    };
    if !page.contains(target) && !page.contains(target_url.path()) {
        return Ok(Err((17, "源地址没有链接到目标".into())));
    }
    let title = html_title(&page).unwrap_or_else(|| source_url.host_str().unwrap_or("pingback").into());
    let excerpt = text::truncate_chars(&text::plain_text(&page), 100);
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0.0.0.0")
        .split(',')
        .next()
        .unwrap_or("0.0.0.0")
        .trim()
        .to_string();
    let status = if site.flag("commentsRequireModeration", false) {
        "waiting"
    } else {
        "approved"
    };
    let id = db
        .insert_feedback(&NewComment {
            cid: post.cid,
            created: now,
            author: text::truncate_chars(&title, 150),
            author_id: 0,
            owner_id: post.author_id,
            mail: String::new(),
            url: text::truncate_chars(source, 255),
            ip: text::trunc_bytes(&ip, 64),
            agent: "pingback".into(),
            text: excerpt,
            status: status.into(),
            kind: "pingback".into(),
            parent: 0,
        })
        .await?;
    Ok(Ok(id))
}

async fn find_public(db: &Db, site: &Site, path: &str, now: i64) -> anyhow::Result<Option<Post>> {
    for hit in site.routes.all_matches(path) {
        let kind = match hit.name.as_str() {
            "post" => "post",
            "page" => "page",
            "attachment" => "attachment",
            _ => continue,
        };
        if let Some(cid) = hit.params.get("cid").and_then(|s| s.parse().ok()) {
            if let Some(post) = db.single_by_cid(cid, kind, now).await? {
                return Ok(Some(post));
            }
        }
        if let Some(slug) = hit.params.get("slug") {
            if let Some(post) = db.single_by_slug(slug, kind, now).await? {
                return Ok(Some(post));
            }
        }
    }
    Ok(None)
}

async fn fetch_http(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?.to_string();
    let port = url.port().unwrap_or(80);
    let path = if url.path().is_empty() { "/" } else { url.path() };
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    let addr = format!("{host}:{port}");
    let mut stream = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .ok()?
    .ok()?;
    let req = format!(
        "GET {path}{query} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: Tiebidli\r\nConnection: close\r\n\r\n"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream.write_all(req.as_bytes()).await.ok()?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            if buf.len() > 200_000 {
                break;
            }
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or(&text);
        Some(body.to_string())
    })
    .await
    .ok()
    .flatten()
}

fn html_title(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let start = lower.find("<title>")? + 7;
    let end = lower[start..].find("</title>")? + start;
    let title = text::plain_text(&page[start..end]);
    let title = title.trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

async fn auth_user(db: &Db, params: &[XVal], user_index: usize) -> Result<SessionUser, String> {
    let name = params.get(user_index).map(|v| v.as_str()).unwrap_or_default();
    let pass = params
        .get(user_index + 1)
        .map(|v| v.as_str())
        .unwrap_or_default();
    match auth::login_user(db, &name, &pass).await {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(fault(403, "用户名或密码错误")),
        Err(err) => Err(fault(500, &err.to_string())),
    }
}

fn require_editor(user: &SessionUser) -> Result<(), String> {
    if user.pass("editor") {
        Ok(())
    } else {
        Err(fault(403, "需要编辑权限"))
    }
}

fn ensure_read(user: &SessionUser, post: &Post) -> Result<(), String> {
    if user.pass("editor") || post.author_id == user.uid || post.status == "publish" {
        Ok(())
    } else {
        Err(fault(403, "没有查看权限"))
    }
}

fn ensure_write(user: &SessionUser, post: &Post) -> Result<(), (i32, String)> {
    if user.pass("editor") || (user.pass("contributor") && post.author_id == user.uid) {
        Ok(())
    } else {
        Err((403, "没有修改权限".into()))
    }
}

fn flag_or(v: Option<&XVal>, default: bool) -> bool {
    match v {
        None => default,
        Some(v) => {
            let s = v.as_str();
            if s == "0" || s.eq_ignore_ascii_case("closed") {
                false
            } else if s == "1" || s.eq_ignore_ascii_case("open") {
                true
            } else {
                v.as_bool()
            }
        }
    }
}

fn post_struct(site: &Site, base: &str, post: &Post) -> String {
    let link = links::abs(base, &links::content_path(site, post));
    let cats = post
        .categories
        .iter()
        .map(|c| val_string(&c.name))
        .collect::<Vec<_>>();
    let keywords = post
        .tags
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    val_struct(&[
        ("postid", val_string(&post.cid.to_string())),
        ("title", val_string(&post.title)),
        ("description", val_string(&post.text)),
        ("link", val_string(&link)),
        ("permaLink", val_string(&link)),
        ("userid", val_string(&post.author_id.to_string())),
        ("wp_slug", val_string(&post.slug)),
        ("post_status", val_string(&wp_status(&post.status))),
        ("dateCreated", val_date(post.created)),
        ("categories", val_array(&cats)),
        ("mt_keywords", val_string(&keywords)),
        (
            "mt_allow_comments",
            val_int(if post.allow_comment { 1 } else { 0 }),
        ),
        ("mt_allow_pings", val_int(if post.allow_ping { 1 } else { 0 })),
    ])
}

fn blog_struct(site: &Site, base: &str, user: &SessionUser) -> String {
    val_struct(&[
        ("blogid", val_string("1")),
        ("blogName", val_string(site.get_or("title", "Tiebidli"))),
        ("url", val_string(base)),
        ("xmlrpc", val_string(&links::abs(base, &links::action_url(site, "/action/xmlrpc", false)))),
        ("isAdmin", val_bool(user.pass("administrator"))),
    ])
}

fn wp_status(status: &str) -> String {
    match status {
        "publish" => "publish",
        "private" => "private",
        "waiting" => "pending",
        _ => "draft",
    }
    .into()
}

fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    for fmt in ["%Y%m%dT%H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp());
        }
    }
    None
}

#[derive(Clone)]
enum XVal {
    Str(String),
    Int(i64),
    Bool(bool),
    Bytes(Vec<u8>),
    Date(String),
    Array(Vec<XVal>),
    Struct(Vec<(String, XVal)>),
}

impl XVal {
    fn as_str(&self) -> String {
        match self {
            XVal::Str(s) | XVal::Date(s) => s.clone(),
            XVal::Int(n) => n.to_string(),
            XVal::Bool(b) => if *b { "1" } else { "0" }.into(),
            XVal::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            _ => String::new(),
        }
    }
    fn as_int(&self) -> i64 {
        match self {
            XVal::Int(n) => *n,
            XVal::Str(s) | XVal::Date(s) => s.trim().parse().unwrap_or(0),
            XVal::Bool(b) => i64::from(*b),
            _ => 0,
        }
    }
    fn as_bool(&self) -> bool {
        match self {
            XVal::Bool(b) => *b,
            XVal::Int(n) => *n != 0,
            XVal::Str(s) => s != "0" && !s.eq_ignore_ascii_case("false") && !s.is_empty(),
            _ => false,
        }
    }
    fn as_bytes(&self) -> Vec<u8> {
        match self {
            XVal::Bytes(b) => b.clone(),
            XVal::Str(s) => decode_b64(s),
            _ => Vec::new(),
        }
    }
    fn as_array(&self) -> Option<&[XVal]> {
        match self {
            XVal::Array(v) => Some(v),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<&XVal> {
        match self {
            XVal::Struct(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct Cur<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cur<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    fn skip(&mut self) {
        let rest = self.rest();
        let n = rest
            .char_indices()
            .take_while(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        self.i += n;
    }
    fn eat(&mut self, lit: &str) -> bool {
        self.skip();
        if self.rest().starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }
    fn until(&mut self, end: &str) -> String {
        if let Some(pos) = self.rest().find(end) {
            let raw = &self.rest()[..pos];
            self.i += pos + end.len();
            decode_entities(raw)
        } else {
            String::new()
        }
    }
}

fn parse_call(body: &str) -> Option<(String, Vec<XVal>)> {
    let start = body.find("<methodName>")? + "<methodName>".len();
    let mut cur = Cur { s: body, i: start };
    let name = cur.until("</methodName>").trim().to_string();
    let mut params = Vec::new();
    loop {
        cur.skip();
        let rest = cur.rest();
        let Some(pos) = rest.find("<param>") else {
            break;
        };
        cur.i += pos + "<param>".len();
        params.push(parse_value(&mut cur));
        cur.eat("</param>");
    }
    Some((name, params))
}

fn parse_value(cur: &mut Cur<'_>) -> XVal {
    cur.skip();
    let wrapped = if cur.rest().starts_with("<value/>") {
        cur.eat("<value/>");
        return XVal::Str(String::new());
    } else {
        cur.eat("<value>")
    };
    cur.skip();
    let v = if cur.eat("<string/>") {
        XVal::Str(String::new())
    } else if cur.eat("<string>") {
        XVal::Str(read_text(cur, "</string>"))
    } else if cur.eat("<int>") {
        XVal::Int(read_text(cur, "</int>").trim().parse().unwrap_or(0))
    } else if cur.eat("<i4>") {
        XVal::Int(read_text(cur, "</i4>").trim().parse().unwrap_or(0))
    } else if cur.eat("<boolean>") {
        XVal::Bool(read_text(cur, "</boolean>").trim() != "0")
    } else if cur.eat("<base64>") {
        XVal::Bytes(decode_b64(&read_text(cur, "</base64>")))
    } else if cur.eat("<double>") {
        XVal::Str(read_text(cur, "</double>"))
    } else if cur.eat("<dateTime.iso8601>") {
        XVal::Date(read_text(cur, "</dateTime.iso8601>"))
    } else if cur.eat("<array>") || cur.eat("<array/>") {
        cur.eat("<data>");
        let mut items = Vec::new();
        loop {
            cur.skip();
            if cur.rest().is_empty()
                || cur.rest().starts_with("</data>")
                || cur.rest().starts_with("</array>")
            {
                break;
            }
            if !cur.rest().starts_with("<value") {
                break;
            }
            items.push(parse_value(cur));
        }
        cur.eat("</data>");
        cur.eat("</array>");
        XVal::Array(items)
    } else if cur.eat("<struct>") || cur.eat("<struct/>") {
        let mut items = Vec::new();
        loop {
            cur.skip();
            if !cur.eat("<member>") {
                break;
            }
            cur.eat("<name>");
            let name = read_text(cur, "</name>");
            let val = parse_value(cur);
            cur.eat("</member>");
            items.push((name.trim().to_string(), val));
        }
        cur.eat("</struct>");
        XVal::Struct(items)
    } else if wrapped {
        XVal::Str(read_text(cur, "</value>"))
    } else {
        XVal::Str(String::new())
    };
    if wrapped {
        cur.eat("</value>");
    }
    v
}

fn read_text(cur: &mut Cur<'_>, end: &str) -> String {
    if cur.rest().starts_with("<![CDATA[") {
        cur.i += "<![CDATA[".len();
        let text = cur.until("]]>");
        let _ = cur.eat(end);
        text
    } else {
        cur.until(end)
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn decode_b64(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let a = val(bytes[i]).unwrap_or(0);
        let b = val(bytes[i + 1]).unwrap_or(0);
        let c = if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            val(bytes[i + 2])
        } else {
            None
        };
        let d = if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
            val(bytes[i + 3])
        } else {
            None
        };
        out.push((a << 2) | (b >> 4));
        if let Some(c) = c {
            out.push((b << 4) | (c >> 2));
            if let Some(d) = d {
                out.push((c << 6) | d);
            }
        }
        i += 4;
    }
    out
}

fn xml_escape(s: &str) -> String {
    html_escape(s)
}

fn val_string(s: &str) -> String {
    format!("<value><string>{}</string></value>", xml_escape(s))
}
fn val_int(n: i64) -> String {
    format!("<value><int>{n}</int></value>")
}
fn val_bool(b: bool) -> String {
    format!("<value><boolean>{}</boolean></value>", if b { 1 } else { 0 })
}
fn val_date(ts: i64) -> String {
    let text = chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y%m%dT%H:%M:%S").to_string())
        .unwrap_or_default();
    format!("<value><dateTime.iso8601>{text}</dateTime.iso8601></value>")
}
fn val_array(items: &[String]) -> String {
    format!("<value><array><data>{}</data></array></value>", items.join(""))
}
fn val_struct(items: &[(&str, String)]) -> String {
    let body = items
        .iter()
        .map(|(k, v)| format!("<member><name>{}</name>{v}</member>", xml_escape(k)))
        .collect::<String>();
    format!("<value><struct>{body}</struct></value>")
}
fn ok_value(v: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><methodResponse><params><param>{v}</param></params></methodResponse>")
}
fn ok_array(items: &[String]) -> String {
    ok_value(&val_array(items))
}
fn fault(code: i32, msg: &str) -> String {
    let body = val_struct(&[
        ("faultCode", val_int(i64::from(code))),
        ("faultString", val_string(msg)),
    ]);
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><methodResponse><fault>{body}</fault></methodResponse>")
}

fn rsd(base: &str, endpoint: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><rsd version="1.0" xmlns="http://archipelago.phrasewise.com/rsd"><service><engineName>Typecho</engineName><engineLink>http://www.typecho.org/</engineLink><homePageLink>{}</homePageLink><apis><api name="WordPress" blogID="1" preferred="true" apiLink="{endpoint}" /><api name="Movable Type" blogID="1" preferred="false" apiLink="{endpoint}" /><api name="MetaWeblog" blogID="1" preferred="false" apiLink="{endpoint}" /><api name="Blogger" blogID="1" preferred="false" apiLink="{endpoint}" /></apis></service></rsd>"#,
        xml_escape(base)
    )
}

fn wlw() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?><manifest xmlns="http://schemas.microsoft.com/wlw/manifest/weblog"><options><supportsKeywords>Yes</supportsKeywords><supportsFileUpload>Yes</supportsFileUpload><supportsExtendedEntries>Yes</supportsExtendedEntries><supportsCustomDate>Yes</supportsCustomDate><supportsCategories>Yes</supportsCategories><supportsPages>Yes</supportsPages><supportsPostAsDraft>Yes</supportsPostAsDraft></options></manifest>"#
}

fn xml_body(body: &str) -> Response {
    text_response(StatusCode::OK, "text/xml; charset=utf-8", body)
}

fn text_response(status: StatusCode, ct: &str, body: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from(body.to_string())))
}
