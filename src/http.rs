//! 公开页面调度。匹配顺序与 Typecho 1.2.1 的路由表一致：某条规则 404 时继续下一条。

use crate::auth;
use crate::comments;
use crate::config::Config;
use crate::db::Db;
use crate::feed::{self, FeedKind};
use crate::links::{self, category_vals};
use crate::model::{Comment, Post, Site};
use crate::render::{Chrome, SingleView};
use crate::repo::{ListFilter, NewComment};
use crate::text::{self, trunc_bytes};
use crate::theme::{self, Renderer};
use crate::write;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tower_http::services::ServeDir;

pub struct AppState {
    pub db: Db,
    pub cfg: Config,
    pub theme: Arc<dyn Renderer>,
}

impl AppState {
    pub fn new(db: Db, cfg: Config) -> Self {
        let theme = theme::JinjaTheme::shared(cfg.template_dir.as_deref());
        Self { db, cfg, theme }
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        .merge(crate::admin::routes())
        .route("/usr/themes/*path", get(theme_asset));
    if let Some(dir) = &state.cfg.uploads_dir {
        router = router.nest_service("/usr/uploads", ServeDir::new(dir));
    }
    router
        .nest_service("/static", ServeDir::new(&state.cfg.static_dir))
        .fallback(dispatch)
        .with_state(state)
}

async fn theme_asset(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let now = chrono::Utc::now().timestamp();
    let Ok(site) = state.db.load_site(now).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "主题文件读取失败");
    };
    let Some(rest_all) = path.strip_prefix("/usr/themes/") else {
        return plain(StatusCode::NOT_FOUND, "主题文件不存在");
    };
    let Some((name, rest)) = rest_all.split_once('/') else {
        return plain(StatusCode::NOT_FOUND, "主题文件不存在");
    };
    if !crate::themes::theme_name_ok(name)
        || rest.split('/').any(|seg| seg == ".." || seg == ".")
        || rest.is_empty()
    {
        return plain(StatusCode::NOT_FOUND, "主题文件不存在");
    }
    let full = crate::themes::locate(&state.cfg, name, &site.theme_name()).join(rest);
    let Ok(bytes) = std::fs::read(&full) else {
        return plain(StatusCode::NOT_FOUND, "主题文件不存在");
    };
    let mime = match full.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "主题文件读取失败"))
}

async fn dispatch(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    match dispatch_inner(&state, req).await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(error = %err, "request failed");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败")
        }
    }
}

async fn dispatch_inner(state: &AppState, req: Request<Body>) -> anyhow::Result<Response> {
    let (parts, body) = req.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => Default::default(),
    };
    let form = if parts.method == Method::POST {
        form_pairs(&String::from_utf8_lossy(&bytes))
    } else {
        HashMap::new()
    };
    let query = form_pairs(parts.uri.query().unwrap_or(""));
    let cookies = cookies_from(&parts.headers);
    let request_path = request_path(parts.uri.path());
    let path = normalize_path(&request_path);
    let now = chrono::Utc::now().timestamp();
    let site = state.db.load_site(now).await?;
    // 内置插件公开路由（启用门控：未启用则 404，继续走正常调度）。
    if path == "/usr/plugins/Sitemap/sitemap.xsl" {
        if !crate::plugins::is_active(&site, "Sitemap") {
            return Ok(plain(StatusCode::NOT_FOUND, "请求的地址不存在"));
        }
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/xsl; charset=utf-8")
            .body(Body::from(crate::sitemap::XSL_BODY))
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败")));
    }
    if path == "/sitemap.xml" {
        if !crate::plugins::is_active(&site, "Sitemap") {
            return Ok(plain(StatusCode::NOT_FOUND, "请求的地址不存在"));
        }
        let (pages, posts) = crate::sitemap::entries(&state.db, now)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let base = links::site_base(&state.cfg.site_url, &site);
        let xml = crate::sitemap::render(&site, &base, &pages, &posts);
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(Body::from(xml))
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败")));
    }
    if path == "/access/log/track.gif" {
        return track_gif(state, &site, &parts.method, &query, &parts.headers, &cookies, &request_path, now).await;
    }
    if path == "/access/ip.json" {
        return access_ip_json(state, &site, &query, &cookies).await;
    }
    if path == "/access/log/delete.json" {
        return access_delete_json(state, &site, &cookies, &bytes).await;
    }
    if path == "/action/handsome-meting-api" {
        return meting_action(state, &site, &query, &cookies).await;
    }
    if path == "/action/cross-edit" {
        return cross_edit_action(state, &site, &parts.method, &query, &form, &parts.headers, &cookies).await;
    }
    // 主题点赞：`action=star_post&cid=` / `action=star_talk&coid=`（GET/POST 均可，纯文本 1/2/-1）。
    if let Some(resp) = star_action(state, &site, &parts.method, &query, &form, &parts.headers, &cookies).await {
        return Ok(resp);
    }
    if path == "/action/xmlrpc" {
        return crate::xmlrpc::handle(
            &state.db,
            &state.cfg,
            &site,
            &parts.method,
            &query,
            &bytes,
            now,
            &parts.headers,
        )
        .await;
    }
    let keyword = if parts.method == Method::POST {
        form.get("s").map(|s| s.trim().to_string())
    } else if parts.method == Method::GET {
        query.get("s").map(|s| s.trim().to_string())
    } else {
        None
    };
    if let Some(kw) = keyword.filter(|s| !s.is_empty()) {
        let mut vals = HashMap::new();
        vals.insert("keywords".into(), links::enc(&kw));
        let raw = site
            .routes
            .url("search", &vals)
            .unwrap_or_else(|| format!("/search/{}/", links::enc(&kw)));
        let dest = links::with_index(&site, &raw);
        return Ok(redirect(&dest, &HeaderMap::new()));
    }
    let (recent_posts, _) = state
        .db
        .list_posts(&ListFilter::Index, 1, 10, now, false)
        .await?;
    let mut archives = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ts in state.db.published_times(now).await? {
        let (year, month, _) = text::ymd(ts, site.timezone());
        if seen.insert((year, month)) {
            archives.push((year, month, ts));
        }
    }
    let bag = ChromeBag {
        recent_posts,
        recent: state.db.recent_comments(5, now).await?,
        archives,
        hot_posts: hot_posts(state, &site, now).await,
        friend_links: friend_links_ten(state, &site).await,
    };
    let root = auth::site_root(
        &state.cfg.site_url,
        &links::site_base(&state.cfg.site_url, &site),
    );
    let viewer = auth::session_user(&state.db, &root, &cookies).await?;
    let viewer_label = viewer.as_ref().map(|user| user.label());
    let viewer_uid = viewer.as_ref().map(|user| user.uid).unwrap_or(0);
    let viewer_auth = viewer.as_ref().map(|user| user.auth_code.clone()).unwrap_or_default();
    // 对齐 PHP `Access_Core::isAdmin`：管理员访问不记日志（浏览数照常按 cookie 去重）。
    let viewer_is_admin = viewer.as_ref().is_some_and(|user| user.pass("administrator"));
    let viewer_is_editor = viewer.as_ref().is_some_and(|user| user.pass("editor"));
    let page_url = absolute_url(&parts.headers, &request_path, &site);
    let ctx = ReqCtx {
        state,
        site: &site,
        bag: &bag,
        now,
        method: &parts.method,
        form: &form,
        query: &query,
        cookies: &cookies,
        headers: &parts.headers,
        original_path: &path,
        request_path: &request_path,
        feed: None,
        comment_page: 0,
        viewer_name: viewer_label.as_deref(),
        viewer_uid,
        viewer_auth: &viewer_auth,
        viewer_is_admin,
        viewer_is_editor,
        page_url: &page_url,
    };
    match walk(&ctx, &path).await? {
        Step::Done(resp) => Ok(resp),
        Step::Next => not_found(&ctx).await,
    }
}

struct ChromeBag {
    recent_posts: Vec<Post>,
    recent: Vec<(Comment, Post)>,
    archives: Vec<(i32, u32, i64)>,
    hot_posts: Vec<Post>,
    friend_links: Vec<crate::friend_links::FriendLink>,
}

/// 侧栏热门：Access 启用时按浏览数，否则按评论数（失败都回空，不阻塞渲染）。
async fn hot_posts(state: &AppState, site: &Site, now: i64) -> Vec<Post> {
    if crate::plugins::is_active(site, "Access") {
        if let Ok(posts) = state.db.hot_posts_by_views(10, now).await {
            return posts;
        }
    }
    state.db.hot_posts(10, now).await.unwrap_or_default()
}

/// 侧栏友链：Links 启用时取全站 `ten` 分组（失败回空）。
async fn friend_links_ten(state: &AppState, site: &Site) -> Vec<crate::friend_links::FriendLink> {
    if !crate::plugins::is_active(site, "Links") {
        return Vec::new();
    }
    crate::friend_links::list_by_sort(&state.db, "ten").await.unwrap_or_default()
}

struct ReqCtx<'a> {
    state: &'a AppState,
    site: &'a Site,
    bag: &'a ChromeBag,
    now: i64,
    method: &'a Method,
    form: &'a HashMap<String, String>,
    query: &'a HashMap<String, String>,
    cookies: &'a HashMap<String, String>,
    headers: &'a HeaderMap,
    original_path: &'a str,
    request_path: &'a str,
    feed: Option<FeedKind>,
    comment_page: i64,
    viewer_name: Option<&'a str>,
    viewer_uid: i64,
    viewer_auth: &'a str,
    viewer_is_admin: bool,
    viewer_is_editor: bool,
    page_url: &'a str,
}

enum Step {
    Done(Response),
    Next,
}

fn walk_boxed<'a>(
    ctx: &'a ReqCtx<'_>,
    path: &'a str,
) -> Pin<Box<dyn Future<Output = anyhow::Result<Step>> + Send + 'a>> {
    Box::pin(walk(ctx, path))
}

async fn walk(ctx: &ReqCtx<'_>, path: &str) -> anyhow::Result<Step> {
    let matches = ctx.site.routes.all_matches(path);
    for hit in matches {
        if ctx.feed.is_some() && matches!(hit.name.as_str(), "feed" | "do" | "feedback") {
            continue;
        }
        match handle(ctx, &hit.name, &hit.params).await? {
            Step::Next => continue,
            Step::Done(resp) => return Ok(Step::Done(maybe_canonical(ctx, &hit.name, &hit.params, resp))),
        }
    }
    if ctx.feed.is_none() {
        if let Some(slug) = archives_slug(path) {
            if let Some(post) = ctx.state.db.single_by_slug(&slug, "post", ctx.now).await? {
                let resp = match show_single(ctx, &post, SingleExtra::default()).await? {
                    Step::Done(resp) => resp,
                    Step::Next => return Ok(Step::Next),
                };
                if ctx.feed.is_some() {
                    return Ok(Step::Done(resp));
                }
                let canonical = links::content_path(ctx.site, &post);
                return Ok(Step::Done(redirect_if_needed(ctx.request_path, &canonical, resp)));
            }
        }
    }
    Ok(Step::Next)
}

/// `Archive::checkPermalink`：首页、评论分页和 404 不跳；其余路径和永久链接不一致时 301。
fn maybe_canonical(ctx: &ReqCtx<'_>, name: &str, params: &HashMap<String, String>, resp: Response) -> Response {
    if ctx.feed.is_some() || matches!(name, "index" | "comment_page" | "do" | "feedback") {
        return resp;
    }
    let Some(canonical) = canonical_of(ctx.site, name, params) else {
        return resp;
    };
    redirect_if_needed(ctx.request_path, &canonical, resp)
}

fn redirect_if_needed(request_path: &str, canonical: &str, resp: Response) -> Response {
    if resp.status() == StatusCode::NOT_FOUND {
        return resp;
    }
    let canonical_path = canonical.split('?').next().unwrap_or(canonical);
    if paths_equal(request_path, canonical_path) {
        resp
    } else {
        redirect_permanent(canonical)
    }
}

fn canonical_of(site: &Site, name: &str, params: &HashMap<String, String>) -> Option<String> {
    if name == "feed" {
        return Some(feed_canonical(site, params.get("feed").map(String::as_str).unwrap_or("")));
    }
    links::route_url(site, name, params)
}

fn feed_canonical(site: &Site, feed_param: &str) -> String {
    let (kind, rest) = split_feed(feed_param);
    let marker = match kind {
        FeedKind::Rdf => "/rss/",
        FeedKind::Atom => "/atom/",
        FeedKind::Rss => "/",
    };
    let rest = rest.trim().trim_matches('/');
    if rest.is_empty() {
        return links::feed_path(site, marker);
    }
    if rest == "comments" {
        let suffix = if marker == "/" {
            "/comments/".to_string()
        } else {
            format!("{marker}comments/")
        };
        return links::feed_path(site, &suffix);
    }
    let base = links::feed_path(site, marker);
    links::common_url(&format!("/{rest}/"), &base)
}

fn paths_equal(request_path: &str, canonical_path: &str) -> bool {
    decode_path(request_path) == decode_path(canonical_path)
}

fn decode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            urlencoding::decode(seg)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| seg.to_string())
        })
        .collect::<Vec<_>>()
        .join("/")
}

async fn handle(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    match name {
        "index" | "index_page" => index_like(ctx, name, params).await,
        "archive" | "archive_page" => {
            let front = ctx.site.get("frontPage");
            if !front.is_empty() && front != "recent" && ctx.site.flag("frontArchive", false) {
                let page = if name == "archive" {
                    1
                } else {
                    page_of(params)
                };
                show_list(
                    ctx,
                    ListFilter::Index,
                    page,
                    ctx.site.get_or("title", "Tiebidli").to_string(),
                    String::new(),
                    "archive",
                    "archive_page",
                    HashMap::new(),
                    true,
                )
                .await
            } else {
                Ok(Step::Next)
            }
        }
        "post" => routed_single(ctx, "post", params).await,
        "page" => routed_single(ctx, "page", params).await,
        "attachment" => routed_single(ctx, "attachment", params).await,
        "category" | "category_page" => category_like(ctx, name, params).await,
        "tag" | "tag_page" => tag_like(ctx, name, params).await,
        "author" | "author_page" => author_like(ctx, name, params).await,
        "search" | "search_page" => search_like(ctx, name, params).await,
        "archive_year" | "archive_year_page" | "archive_month" | "archive_month_page"
        | "archive_day" | "archive_day_page" => date_like(ctx, name, params).await,
        "comment_page" => {
            let perm = params.get("permalink").cloned().unwrap_or_default();
            let path = leading_slash(&perm);
            let requested = params
                .get("commentPage")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            let mut inner = *ctx;
            inner.comment_page = requested;
            match find_single(&inner, &path).await? {
                Some(post) => show_single(&inner, &post, SingleExtra::default()).await,
                None => Ok(Step::Next),
            }
        }
        "feed" => feed_like(ctx, params).await,
        "feedback" => {
            let kind = params.get("type").map(String::as_str).unwrap_or("");
            let perm = params.get("permalink").cloned().unwrap_or_default();
            let path = leading_slash(&perm);
            if kind == "comment" && *ctx.method == Method::POST {
                post_comment(ctx, &path).await
            } else if kind == "trackback" {
                post_trackback(ctx, &path).await
            } else {
                Ok(Step::Next)
            }
        }
        _ => Ok(Step::Next),
    }
}

async fn index_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    if ctx.feed.is_none() {
        let front = ctx.site.get("frontPage");
        if let Some(raw) = front.strip_prefix("page:") {
            let cid: i64 = raw.parse().unwrap_or(0);
            if let Some(post) = ctx.state.db.single_by_cid(cid, "page", ctx.now).await? {
                return show_single(ctx, &post, SingleExtra::default()).await;
            }
            return Ok(Step::Next);
        }
        if front.starts_with("file:") {
            return not_found(ctx).await.map(Step::Done);
        }
    }
    let page = if name == "index" { 1 } else { page_of(params) };
    show_list(
        ctx,
        ListFilter::Index,
        page,
        ctx.site.get_or("title", "Tiebidli").to_string(),
        String::new(),
        "index",
        "index_page",
        HashMap::new(),
        true,
    )
    .await
}

async fn routed_single(
    ctx: &ReqCtx<'_>,
    kind: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let Some(post) = load_routed(ctx, kind, params).await? else {
        return Ok(Step::Next);
    };
    show_single(ctx, &post, SingleExtra::default()).await
}

async fn load_routed(
    ctx: &ReqCtx<'_>,
    kind: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Option<Post>> {
    let post = if let Some(cid) = params.get("cid").and_then(|s| s.parse::<i64>().ok()) {
        ctx.state.db.single_by_cid(cid, kind, ctx.now).await?
    } else if let Some(slug) = params.get("slug") {
        ctx.state.db.single_by_slug(slug, kind, ctx.now).await?
    } else {
        None
    };
    if let (Some(post), Some(slug)) = (post.as_ref(), params.get("slug")) {
        if &post.slug != slug {
            return Ok(None);
        }
    }
    if let (Some(post), Some(cid)) = (
        post.as_ref(),
        params.get("cid").and_then(|s| s.parse::<i64>().ok()),
    ) {
        if post.cid != cid {
            return Ok(None);
        }
    }
    if let Some(post) = post.as_ref() {
        if let Some(year) = params.get("year").and_then(|s| s.parse::<i32>().ok()) {
            let month = params
                .get("month")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let day = params.get("day").and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Some((start, end)) = text::calendar_range(year, month, day, ctx.site.timezone())
            {
                if post.created < start || post.created >= end {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }
        if let Some(cat) = params.get("category") {
            let slug = post
                .categories
                .first()
                .map(|c| c.slug.as_str())
                .unwrap_or("");
            if cat != slug && cat != &links::enc(slug) {
                return Ok(None);
            }
        }
        if let Some(dir) = params.get("directory") {
            let vals = links::post_values(ctx.site, post);
            let raw = vals.get("directory_raw").map(String::as_str).unwrap_or("");
            let encoded = vals.get("directory").map(String::as_str).unwrap_or("");
            if dir != raw && dir != encoded {
                return Ok(None);
            }
        }
    }
    Ok(post)
}

async fn find_single(ctx: &ReqCtx<'_>, path: &str) -> anyhow::Result<Option<Post>> {
    let path = normalize_path(path);
    for hit in ctx.site.routes.all_matches(&path) {
        let kind = match hit.name.as_str() {
            "post" => "post",
            "page" => "page",
            "attachment" => "attachment",
            _ => continue,
        };
        if let Some(post) = load_routed(ctx, kind, &hit.params).await? {
            return Ok(Some(post));
        }
    }
    if let Some(slug) = archives_slug(&path) {
        return Ok(ctx.state.db.single_by_slug(&slug, "post", ctx.now).await?);
    }
    Ok(None)
}

async fn category_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let Some(meta) = resolve_meta(ctx, "category", params).await? else {
        return Ok(Step::Next);
    };
    if let Some(dir) = params.get("directory") {
        let vals = category_vals(ctx.site, &meta);
        let raw = vals.get("directory_raw").map(String::as_str).unwrap_or("");
        let encoded = vals.get("directory").map(String::as_str).unwrap_or("");
        if dir != raw && dir != encoded {
            return Ok(Step::Next);
        }
    }
    let mids = links::child_mids(&ctx.site.categories, meta.mid);
    let page = if name.ends_with("_page") {
        page_of(params)
    } else {
        1
    };
    let vals = category_vals(ctx.site, &meta);
    let heading = format!("分类 {} 下的文章", meta.name);
    let sub = meta.description.clone();
    show_list(
        ctx,
        ListFilter::Category(mids),
        page,
        heading,
        sub,
        "category",
        "category_page",
        vals,
        true,
    )
    .await
}

async fn tag_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let Some(meta) = resolve_meta(ctx, "tag", params).await? else {
        return Ok(Step::Next);
    };
    let page = if name.ends_with("_page") {
        page_of(params)
    } else {
        1
    };
    let mut vals = HashMap::new();
    vals.insert("slug".into(), links::enc(&meta.slug));
    let heading = format!("标签 {} 下的文章", meta.name);
    let sub = meta.description.clone();
    let mid = meta.mid;
    show_list(
        ctx,
        ListFilter::Tag(mid),
        page,
        heading,
        sub,
        "tag",
        "tag_page",
        vals,
        true,
    )
    .await
}

async fn resolve_meta(
    ctx: &ReqCtx<'_>,
    kind: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Option<crate::model::Meta>> {
    if let Some(mid) = params.get("mid").and_then(|s| s.parse::<i64>().ok()) {
        let found = match kind {
            "category" => ctx.site.categories.iter().find(|m| m.mid == mid).cloned(),
            _ => ctx.site.tags.iter().find(|m| m.mid == mid).cloned(),
        };
        if found.is_some() {
            return Ok(found);
        }
    }
    let slug = if let Some(slug) = params.get("slug") {
        slug.clone()
    } else if let Some(dir) = params.get("directory") {
        dir.split('/').next_back().unwrap_or("").to_string()
    } else {
        return Ok(None);
    };
    if slug.is_empty() {
        return Ok(None);
    }
    Ok(ctx.state.db.meta_by_slug(kind, &slug).await?)
}

async fn author_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let Some(uid) = params.get("uid").and_then(|s| s.parse::<i64>().ok()) else {
        return Ok(Step::Next);
    };
    let Some(author) = ctx.state.db.author_name(uid).await? else {
        return Ok(Step::Next);
    };
    let page = if name.ends_with("_page") {
        page_of(params)
    } else {
        1
    };
    let mut vals = HashMap::new();
    vals.insert("uid".into(), uid.to_string());
    show_list(
        ctx,
        ListFilter::Author(uid),
        page,
        format!("{author} 发布的文章"),
        String::new(),
        "author",
        "author_page",
        vals,
        true,
    )
    .await
}

async fn search_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let keyword = params.get("keywords").cloned().unwrap_or_default();
    let page = if name.ends_with("_page") {
        page_of(params)
    } else {
        1
    };
    let page_size = if ctx.feed.is_some() {
        10
    } else {
        ctx.site.page_size()
    };
    let (posts, total) = ctx
        .state
        .db
        .list_posts(
            &ListFilter::Search(keyword.clone()),
            page,
            page_size,
            ctx.now,
            ctx.feed.is_some(),
        )
        .await?;
    if page > 1 && posts.is_empty() {
        return Ok(Step::Next);
    }
    if let Some(kind) = ctx.feed {
        let title = format!("包含关键字 {keyword} 的文章");
        return Ok(Step::Done(xml_response(
            kind,
            feed::render_posts(
                kind,
                ctx.site,
                &base_of(ctx),
                &title,
                ctx.site.get("description"),
                ctx.original_path,
                &posts,
                ctx.site.flag("feedFullText", true),
            ),
        )));
    }
    let set_ep = log_archive_visit(ctx, None, None).await;
    let chrome = chrome(ctx, ctx.cookies.clone());
    let html = ctx
        .state
        .theme
        .render_search(&chrome, &keyword, &posts, page, total)?;
    let mut headers = HeaderMap::new();
    attach_entrypoint(&mut headers, set_ep);
    Ok(Step::Done(html_response(
        ctx,
        StatusCode::OK,
        html,
        &headers,
    )))
}

async fn date_like(
    ctx: &ReqCtx<'_>,
    name: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<Step> {
    let Some(year) = params.get("year").and_then(|s| s.parse::<i32>().ok()) else {
        return Ok(Step::Next);
    };
    let month = if name.contains("month") || name.contains("day") {
        params
            .get("month")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let day = if name.contains("day") {
        params.get("day").and_then(|s| s.parse().ok()).unwrap_or(0)
    } else {
        0
    };
    let Some((start, end)) = text::calendar_range(year, month, day, ctx.site.timezone()) else {
        return Ok(Step::Next);
    };
    let page = if name.ends_with("_page") {
        page_of(params)
    } else {
        1
    };
    let (base_name, page_name) = if day > 0 {
        ("archive_day", "archive_day_page")
    } else if month > 0 {
        ("archive_month", "archive_month_page")
    } else {
        ("archive_year", "archive_year_page")
    };
    let mut vals = HashMap::new();
    vals.insert("year".into(), format!("{year:04}"));
    if month > 0 {
        vals.insert("month".into(), format!("{month:02}"));
    }
    if day > 0 {
        vals.insert("day".into(), format!("{day:02}"));
    }
    let heading = if day > 0 {
        format!("{year}年{month}月{day}日")
    } else if month > 0 {
        format!("{year}年{month}月")
    } else {
        format!("{year}年")
    };
    show_list(
        ctx,
        ListFilter::Date { start, end },
        page,
        heading,
        String::new(),
        base_name,
        page_name,
        vals,
        true,
    )
    .await
}

async fn show_list(
    ctx: &ReqCtx<'_>,
    filter: ListFilter,
    page: i64,
    heading: String,
    sub: String,
    base_name: &str,
    page_name: &str,
    vals: HashMap<String, String>,
    show_cloud: bool,
) -> anyhow::Result<Step> {
    let page_size = if ctx.feed.is_some() {
        10
    } else {
        ctx.site.page_size()
    };
    let (posts, total) = ctx
        .state
        .db
        .list_posts(&filter, page, page_size, ctx.now, ctx.feed.is_some())
        .await?;
    if page > 1 && posts.is_empty() {
        return Ok(Step::Next);
    }
    if let Some(kind) = ctx.feed {
        return Ok(Step::Done(xml_response(
            kind,
            feed::render_posts(
                kind,
                ctx.site,
                &base_of(ctx),
                &heading,
                if sub.is_empty() {
                    ctx.site.get("description")
                } else {
                    sub.as_str()
                },
                ctx.original_path,
                &posts,
                ctx.site.flag("feedFullText", true),
            ),
        )));
    }
    // 后端模式覆盖全部 Archive（对齐 `Widget_Archive::beforeRender`）：首页 mid 记 0，
    // 分类/标签记首个 mid，其余留空。管理员不记。
    let access_mid = match &filter {
        ListFilter::Index => Some(0),
        ListFilter::Category(mids) => mids.first().copied(),
        ListFilter::Tag(mid) => Some(*mid),
        _ => None,
    };
    let set_ep = log_archive_visit(ctx, None, access_mid).await;
    let mut chrome = chrome(ctx, ctx.cookies.clone());
    chrome.access_ids = (None, access_mid);
    let html = ctx.state.theme.render_list(
        &chrome,
        &heading,
        &sub,
        &posts,
        page,
        total,
        base_name,
        page_name,
        &vals,
        show_cloud,
    )?;
    let mut headers = HeaderMap::new();
    attach_entrypoint(&mut headers, set_ep);
    Ok(Step::Done(html_response(
        ctx,
        StatusCode::OK,
        html,
        &headers,
    )))
}

struct SingleExtra<'a> {
    password_error: &'a str,
    comment_error: &'a str,
    cookies: Option<HashMap<String, String>>,
    remember_author: Option<&'a str>,
    remember_mail: Option<&'a str>,
    remember_url: Option<&'a str>,
    remember_text: Option<&'a str>,
    status: Option<StatusCode>,
    extra: HeaderMap,
}

impl Default for SingleExtra<'static> {
    fn default() -> Self {
        Self {
            password_error: "",
            comment_error: "",
            cookies: None,
            remember_author: None,
            remember_mail: None,
            remember_url: None,
            remember_text: None,
            status: None,
            extra: HeaderMap::new(),
        }
    }
}

async fn show_single(
    ctx: &ReqCtx<'_>,
    post: &Post,
    extra: SingleExtra<'_>,
) -> anyhow::Result<Step> {
    if let Some(kind) = ctx.feed {
        if post.has_password() || !post.allow_feed {
            return Ok(Step::Next);
        }
        let comments = ctx.state.db.comments_for(post.cid, "", "", true).await?;
        let rows: Vec<(Comment, Post)> = comments
            .into_iter()
            .take(10)
            .map(|c| (c, post.clone()))
            .collect();
        let title = format!(
            "{} - {} 的评论",
            ctx.site.get_or("title", "Tiebidli"),
            post.title
        );
        let xml = feed::render_comments(
            kind,
            ctx.site,
            &base_of(ctx),
            &title,
            ctx.original_path,
            &rows,
            ctx.site.flag("commentsMarkdown", false),
        );
        return Ok(Step::Done(xml_response(kind, xml)));
    }

    let mut cookies = extra.cookies.unwrap_or_else(|| ctx.cookies.clone());
    let mut headers = extra.extra;
    let mut password_error = extra.password_error.to_string();
    let protect = protect_key(ctx, post.cid);
    if *ctx.method == Method::POST && extra.comment_error.is_empty() && password_error.is_empty() {
        if let Some(pw) = ctx.form.get("protectPassword") {
            let cid = ctx
                .form
                .get("protectCID")
                .and_then(|s| s.parse().ok())
                .unwrap_or(post.cid);
            if cid == post.cid {
                cookies.insert(protect.clone(), pw.clone());
                push_cookie(&mut headers, &protect, pw, None);
                if post.has_password() && pw != &post.password {
                    password_error = "对不起,您输入的密码错误".into();
                }
            }
        }
    }
    let locked = post.has_password()
        && cookies
            .get(&protect)
            .map(|v| v != &post.password)
            .unwrap_or(true);
    if post.kind == "attachment" && !locked && ctx.query.contains_key("download") {
        return Ok(Step::Done(attachment_download(ctx, post)));
    }
    let author = extra
        .remember_author
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            cookies
                .get(&remember_key(ctx, "__typecho_remember_author"))
                .cloned()
                .unwrap_or_default()
        });
    let mail = extra
        .remember_mail
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            cookies
                .get(&remember_key(ctx, "__typecho_remember_mail"))
                .cloned()
                .unwrap_or_default()
        });
    let url = extra
        .remember_url
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            cookies
                .get(&remember_key(ctx, "__typecho_remember_url"))
                .cloned()
                .unwrap_or_default()
        });
    let text = extra
        .remember_text
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            cookies
                .get(&remember_key(ctx, "__typecho_remember_text"))
                .cloned()
                .unwrap_or_default()
        });
    let all = if locked {
        Vec::new()
    } else {
        ctx.state
            .db
            .comments_for(post.cid, &author, &mail, false)
            .await?
    };
    let comment_page = comments::page_of(ctx.site, all, ctx.comment_page);
    let comment_nav = comments::nav_html(ctx.site, post, &comment_page);
    let ping_html = comments::ping_list_html(&comment_page.pings);
    let attachment_html = if post.kind == "attachment" {
        write::attachment_markup(&base_of(ctx), &post.text)
    } else {
        String::new()
    };
    let (newer, older) = if post.kind == "post" {
        ctx.state.db.neighbors(post, ctx.now).await?
    } else {
        (None, None)
    };
    let mut chrome = chrome(ctx, cookies.clone());
    chrome.access_ids = (Some(post.cid), None);
    let reply_to = ctx
        .query
        .get("replyTo")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let view = SingleView {
        post,
        locked,
        password_error: &password_error,
        comment_error: extra.comment_error,
        comments: &comment_page.comments,
        newer: newer.as_ref(),
        older: older.as_ref(),
        remember_author: &author,
        remember_mail: &mail,
        remember_url: &url,
        remember_text: &text,
        reply_to,
        comment_nav: &comment_nav,
        ping_html: &ping_html,
        attachment_html: &attachment_html,
        stars: crate::stars::star_count(&ctx.state.db, &ctx.state.db.table("contents"), "cid", post.cid).await,
        cross_json: if post.kind == "page" && post.slug == "cross" {
            Some(crate::render::cross_stream_json(&ctx.state.db, &chrome, post, &comment_page.comments).await)
        } else {
            None
        },
    };
    let html = ctx.state.theme.render_single(&chrome, &view)?;
    let status = extra.status.unwrap_or(if locked {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::OK
    });
    // 浏览数：与主题 `get_post_view` 一致，仅单页 GET、未上锁时按 cookie 去重递增。
    // 后端写入模式（writeType=0）同时记一条访问日志，前端模式靠 track.gif。
    if !locked
        && *ctx.method == Method::GET
        && matches!(post.kind.as_str(), "post" | "page")
    {
        let root = root_of(ctx);
        let cookie_name = crate::auth::cookie_name(&root, crate::access::VIEWS_COOKIE);
        let seen = cookies
            .get(&cookie_name)
            .map(String::as_str)
            .unwrap_or("");
        if !crate::access::already_viewed(seen, post.cid) {
            if let Ok(n) = crate::access::increment_views(&ctx.state.db, post.cid).await {
                let _ = n;
            }
            let next = crate::access::mark_viewed(seen, post.cid);
            push_cookie(&mut headers, &cookie_name, &next, None);
        }
        // 后端模式记一条访问日志（cid 为当前内容），前端模式靠打点，不双写。
        attach_entrypoint(&mut headers, log_archive_visit(ctx, Some(post.cid), None).await);
    }
    Ok(Step::Done(html_response(ctx, status, html, &headers)))
}

/// 主题点赞（对齐主题 `Star::starCommon`）：cookie 去重，成功 `1`、已赞 `2`、失败 `-1`。
/// 非点赞请求返回 `None`，继续正常调度。
async fn star_action(
    state: &AppState,
    site: &crate::model::Site,
    method: &Method,
    query: &HashMap<String, String>,
    form: &HashMap<String, String>,
    headers: &HeaderMap,
    cookies: &HashMap<String, String>,
) -> Option<Response> {
    if *method != Method::GET && *method != Method::POST {
        return None;
    }
    let action = query.get("action").or_else(|| form.get("action"))?;
    let (id_key, table, id_col, cookie_key) = match action.as_str() {
        "star_post" => ("cid", "contents", "cid", "extend_post_stars"),
        "star_talk" => ("coid", "comments", "coid", "extend_say_stars"),
        _ => return None,
    };
    let fail = || plain(StatusCode::OK, "-1");
    let id: i64 = query
        .get(id_key)
        .or_else(|| form.get(id_key))
        .and_then(|s| s.parse().ok())
        .filter(|id| *id > 0)?;
    crate::stars::ensure_stars_columns(&state.db).await;
    let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, site));
    let name = crate::auth::cookie_name(&root, cookie_key);
    let seen = cookies.get(&name).map(String::as_str).unwrap_or("");
    if crate::stars::already_starred(seen, id) {
        return Some(plain(StatusCode::OK, "2"));
    }
    let table_name = state.db.table(if table == "contents" { "contents" } else { "comments" });
    if crate::stars::star_once(&state.db, &table_name, id_col, id).await.is_none() {
        return Some(fail());
    }
    let mut headers_out = HeaderMap::new();
    push_cookie(&mut headers_out, &name, &crate::stars::mark_starred(seen, id), None);
    let mut resp = plain(StatusCode::OK, "1");
    for (k, v) in headers_out.iter() {
        resp.headers_mut().append(k, v.clone());
    }
    let _ = headers;
    Some(resp)
}

/// 给 `admin::action_dispatch` 用的主题音乐入口（公开路由，自带参数校验）。
pub(crate) async fn meting_route(state: &AppState, query: &HashMap<String, String>, headers: &HeaderMap) -> Response {
    let now = chrono::Utc::now().timestamp();
    let Ok(site) = state.db.load_site(now).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    let cookies = cookies_from(headers);
    meting_action(state, &site, query, &cookies).await.unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"))
}

/// 给 `admin::action_dispatch` 用的时光机编辑入口（自带登录 + contributor + `_` 校验）。
pub(crate) async fn cross_edit_route(
    state: &AppState,
    method: &Method,
    query: &HashMap<String, String>,
    body: &str,
    headers: &HeaderMap,
) -> Response {
    let now = chrono::Utc::now().timestamp();
    let Ok(site) = state.db.load_site(now).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    let form = form_pairs(body);
    let cookies = cookies_from(headers);
    cross_edit_action(state, &site, method, query, &form, headers, &cookies)
        .await
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"))
}

/// 主题音乐 Action（路径与现网 LocalConst `MUSIC_API` 一致）。
/// 参数校验对齐 `MetingAction::check`；目前只实现 `netease` 源。
async fn meting_action(
    state: &AppState,
    site: &crate::model::Site,
    query: &HashMap<String, String>,
    cookies: &HashMap<String, String>,
) -> anyhow::Result<Response> {
    let _ = state;
    if query.get("do").map(String::as_str) == Some("parse") {
        // 对齐 `MetingAction::action`：`do=parse` 需登录。
        let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, site));
        let logged = auth::session_user(&state.db, &root, cookies).await.ok().flatten().is_some();
        if !logged {
            return Ok(plain(StatusCode::FORBIDDEN, "forbidden"));
        }
        let data = query.get("data").map(String::as_str).unwrap_or("");
        let (server, id, kind) = crate::meting::parse_music_url(data);
        if server.is_empty() {
            return Ok(plain(
                StatusCode::OK,
                format!("[hplayer]\n[Music title=\"歌曲名\" author=\"歌手\" url=\"{data}\" pic=\"图片文件URL\" lrc=\"歌词文件URL\"/]\n[/hplayer]\n").as_str(),
            ));
        }
        return Ok(plain(
            StatusCode::OK,
            format!("[hplayer]\n[Music server=\"{server}\" id=\"{id}\" type=\"{kind}\"/]\n[/hplayer]\n").as_str(),
        ));
    }
    let server = query.get("server").map(String::as_str).unwrap_or("");
    let kind = query.get("type").map(String::as_str).unwrap_or("");
    let id = query.get("id").map(String::as_str).unwrap_or("");
    if !crate::meting::check(server, kind, id) || server != "netease" {
        return Ok(plain(StatusCode::FORBIDDEN, ""));
    }
    let body = match kind {
        "song" => crate::meting::api_song(id).await.map(|t| serde_json::to_string(&t).unwrap_or_default()),
        "playlist" => crate::meting::api_playlist(id).await.map(|t| serde_json::to_string(&t).unwrap_or_default()),
        "artist" => crate::meting::api_artist(id).await.map(|t| serde_json::to_string(&t).unwrap_or_default()),
        "album" => crate::meting::api_album(id).await.map(|t| serde_json::to_string(&t).unwrap_or_default()),
        "search" => crate::meting::api_search(id).await.map(|t| serde_json::to_string(&t).unwrap_or_default()),
        "url" => crate::meting::api_url(id).await.map(|v| v.to_string()),
        "lrc" => crate::meting::api_lrc(id).await.map(|v| v.to_string()),
        "pic" => crate::meting::api_pic(id).await.map(|v| v.to_string()),
        _ => None,
    };
    match body {
        Some(json) => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(Body::from(json))
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"))),
        None => Ok(plain(StatusCode::BAD_GATEWAY, "music api error")),
    }
}

/// 时光机评论编辑/删除（对齐主题 `CrossAction` 的登录 + contributor 门禁与 `_` 校验）。
/// 现网把编辑入口直接渲进 HTML（未登录点开才 302），这里同样只做 API 门禁。
async fn cross_edit_action(
    state: &AppState,
    site: &crate::model::Site,
    method: &Method,
    query: &HashMap<String, String>,
    form: &HashMap<String, String>,
    headers: &HeaderMap,
    cookies: &HashMap<String, String>,
) -> anyhow::Result<Response> {
    let fail = || json_response(serde_json::json!({"code": 100, "data": "forbidden"}));
    if *method != Method::POST {
        return Ok(fail());
    }
    let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, site));
    let user = auth::session_user(&state.db, &root, cookies).await.ok().flatten();
    let Some(user) = user.filter(|u| u.pass("contributor")) else {
        return Ok(fail());
    };
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok()).unwrap_or("");
    let given = query.get("_").or_else(|| form.get("_")).map(String::as_str).unwrap_or("");
    let token =
        auth::security_token(site.get("secret"), Some((user.auth_code.as_str(), user.uid)), referer);
    if given != token {
        return Ok(fail());
    }
    let coid: i64 = query
        .get("coid")
        .or_else(|| form.get("coid"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if coid <= 0 {
        return Ok(fail());
    }
    let can = state
        .db
        .comment_author(coid)
        .await
        .is_ok_and(|author| author == user.uid || user.pass("editor"));
    if !can {
        return Ok(fail());
    }
    let op = query.get("do").or_else(|| form.get("do")).map(String::as_str).unwrap_or("");
    let ok = match op {
        "delete" => state.db.delete_comment(coid).await.is_ok(),
        "edit" => {
            let text = form.get("text").map(String::as_str).unwrap_or("");
            !text.trim().is_empty() && state.db.update_comment_text(coid, text).await.is_ok()
        }
        _ => false,
    };
    if ok {
        Ok(json_response(serde_json::json!({"code": 0})))
    } else {
        Ok(fail())
    }
}

/// `GET /access/log/track.gif`：启用门控，始终回 1x1 GIF。
/// query 对齐参考：`u` 受访地址、`cid` 内容 id、`mid` 分类 id、`rand` 随机数（仅防缓存）。
/// 入库语义对齐 PHP `Action::writeLogs`：仅前端写入模式（`writeType=1`）且非管理员才写库；
/// 后端模式（默认）只回像素，写库在文章/页面渲染路径（`Plugin::backend` 对应处）。
async fn track_gif(
    state: &AppState,
    site: &crate::model::Site,
    method: &Method,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    cookies: &HashMap<String, String>,
    request_path: &str,
    now: i64,
) -> anyhow::Result<Response> {
    if *method != Method::GET || !crate::plugins::is_active(site, "Access") {
        return Ok(plain(StatusCode::NOT_FOUND, "请求的地址不存在"));
    }
    let write_type = crate::plugins::plugin_setting(site, "Access", "writeType", "0");
    // 前端写库路径同样解析 entrypoint：外链首次访问把 cookie 带回像素响应。
    let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, site));
    let ep_name = crate::auth::cookie_name(&root, crate::access::ENTRYPOINT_COOKIE);
    let set_ep: Option<(String, String)> = if write_type.trim() == "1" {
        // 表可能被外部删掉（如 isDrop=0 后又手动启用），best-effort 补上，保证打点不 500。
        let _ = crate::access::ensure_table(&state.db).await;
        if !access_admin(state, site, cookies).await {
            let url = query.get("u").map(String::as_str).unwrap_or(request_path);
            let cid = query.get("cid").and_then(|s| s.parse::<i64>().ok());
            let mid = query.get("mid").and_then(|s| s.parse::<i64>().ok());
            let ua = headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let referer = headers
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let cookie_ep = cookies.get(&ep_name).map(String::as_str).unwrap_or("");
            let (entrypoint, needs_set) = crate::access::resolve_entrypoint(
                &links::site_base(&state.cfg.site_url, site),
                referer,
                cookie_ep,
            );
            let _ = crate::access::log_visit(
                &state.db,
                ua,
                url,
                &client_ip(headers),
                referer,
                &entrypoint,
                now,
                cid,
                mid,
            )
            .await;
            needs_set.then(|| (ep_name.clone(), entrypoint))
        } else {
            None
        }
    } else {
        None
    };
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/gif")
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .body(Body::from(crate::access::TRACK_GIF))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"));
    if let Some((name, value)) = set_ep {
        if let Some(v) = cookie_header_value(&name, &value, None) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    Ok(resp)
}

/// Access JSON 接口的管理员门禁，对齐 PHP `Access_Action::checkAuth`。
async fn access_admin(
    state: &AppState,
    site: &crate::model::Site,
    cookies: &HashMap<String, String>,
) -> bool {
    let root = auth::site_root(
        &state.cfg.site_url,
        &links::site_base(&state.cfg.site_url, site),
    );
    auth::session_user(&state.db, &root, cookies)
        .await
        .ok()
        .flatten()
        .is_some_and(|user| user.pass("administrator"))
}

fn json_response(value: serde_json::Value) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(value.to_string()))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"))
}

/// `GET /access/ip.json`：仅管理员。对齐 PHP `Access_Action::ip`：
/// 本地可判定的（回环/内网等）直接回 `code 0`；公网走 KeyCDN，成功 `code 0`，
/// 不可达回 `code 100`（与 PHP 相近文案），不假造地理数据。
async fn access_ip_json(
    state: &AppState,
    site: &crate::model::Site,
    query: &HashMap<String, String>,
    cookies: &HashMap<String, String>,
) -> anyhow::Result<Response> {
    if !crate::plugins::is_active(site, "Access") {
        return Ok(plain(StatusCode::NOT_FOUND, "请求的地址不存在"));
    }
    if !access_admin(state, site, cookies).await {
        return Ok(json_response(serde_json::json!({"code": 100, "data": "Access Denied"})));
    }
    let raw = query.get("ip").map(String::as_str).unwrap_or("");
    let (code, data) = crate::access::geo_for_ip(raw).await;
    Ok(json_response(serde_json::json!({"code": code, "data": data})))
}

/// `/access/log/delete.json`：仅管理员，JSON 体为 id 数组或 `{"ids": [...]}`，
/// 对齐 PHP `Action::deleteLogs`（读原始输入、逐个删除、回 `code`）。
async fn access_delete_json(
    state: &AppState,
    site: &crate::model::Site,
    cookies: &HashMap<String, String>,
    body: &[u8],
) -> anyhow::Result<Response> {
    if !crate::plugins::is_active(site, "Access") {
        return Ok(plain(StatusCode::NOT_FOUND, "请求的地址不存在"));
    }
    if !access_admin(state, site, cookies).await {
        return Ok(json_response(serde_json::json!({"code": 100, "data": "Access Denied"})));
    }
    let ids: Vec<i64> = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
            .collect(),
        Ok(serde_json::Value::Object(map)) => match map.get("ids") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
                .collect(),
            _ => {
                return Ok(json_response(
                    serde_json::json!({"code": 100, "data": "params invalid"}),
                ))
            }
        },
        _ => {
            return Ok(json_response(serde_json::json!({"code": 100, "data": "params invalid"})))
        }
    };
    match crate::access::delete_logs(&state.db, &ids).await {
        Ok(_) => Ok(json_response(serde_json::json!({"code": 0}))),
        Err(_) => Ok(json_response(serde_json::json!({"code": 100, "data": "删除失败"}))),
    }
}

async fn feed_like(ctx: &ReqCtx<'_>, params: &HashMap<String, String>) -> anyhow::Result<Step> {
    let raw = params.get("feed").cloned().unwrap_or_default();
    let (kind, rest) = split_feed(&raw);
    let rest = normalize_path(&leading_slash(rest.trim()));
    // 对齐 PHP `Widget_Archive->beforeRender`：任意 Archive（含 feed）都记一条归档日志
    //（`parseArchive` 未识别 feed，cid/mid 为空）。管理员跳过、前端模式不双写照旧。
    if rest == "/comments" {
        let rows = ctx.state.db.recent_comments(10, ctx.now).await?;
        let title = format!("{} 的评论", ctx.site.get_or("title", "Tiebidli"));
        let xml = feed::render_comments(
            kind,
            ctx.site,
            &base_of(ctx),
            &title,
            ctx.original_path,
            &rows,
            ctx.site.flag("commentsMarkdown", false),
        );
        let mut extra = HeaderMap::new();
        attach_entrypoint(&mut extra, log_archive_visit(ctx, None, None).await);
        return Ok(Step::Done(xml_response_with(kind, xml, &extra)));
    }
    let inner = ReqCtx {
        feed: Some(kind),
        original_path: ctx.original_path,
        ..*ctx
    };
    let target = if rest == "/" { "/" } else { rest.as_str() };
    match walk_boxed(&inner, target).await? {
        Step::Done(mut resp) => {
            if let Some((name, value)) = log_archive_visit(ctx, None, None).await {
                if let Some(v) = cookie_header_value(&name, &value, None) {
                    resp.headers_mut().append(header::SET_COOKIE, v);
                }
            }
            Ok(Step::Done(resp))
        }
        // 聚合 feed 未命中由 404 归档记（`not_found` 内已记），这里不再记，避免双写。
        Step::Next => not_found(ctx).await.map(Step::Done),
    }
}

fn attachment_download(ctx: &ReqCtx<'_>, post: &Post) -> Response {
    let Some((name, path, mime)) = write::attachment_fields(&post.text) else {
        return plain(StatusCode::NOT_FOUND, "附件不存在");
    };
    let rel = path
        .trim_start_matches('/')
        .trim_start_matches("usr/uploads/")
        .trim_start_matches('/');
    if rel.contains("..") || rel.is_empty() {
        return plain(StatusCode::NOT_FOUND, "附件不存在");
    }
    let root = ctx
        .state
        .cfg
        .uploads_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("usr/uploads"));
    match std::fs::read(root.join(rel)) {
        Ok(bytes) => {
            let disp = format!("attachment; filename=\"{}\"", name.replace('"', ""));
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CONTENT_DISPOSITION, disp)
                .body(Body::from(bytes))
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "附件读取失败"))
        }
        Err(_) => plain(StatusCode::NOT_FOUND, "附件文件不存在"),
    }
}

async fn post_trackback(ctx: &ReqCtx<'_>, path: &str) -> anyhow::Result<Step> {
    let Some(post) = find_single(ctx, path).await? else {
        return Ok(Step::Next);
    };
    if *ctx.method != Method::POST || header_nonempty(ctx.headers, header::REFERER) {
        return Ok(Step::Done(redirect(
            &links::content_path(ctx.site, &post),
            &HeaderMap::new(),
        )));
    }
    if !post.allow_ping {
        return Ok(Step::Done(plain(StatusCode::FORBIDDEN, "对不起,此内容的引用被禁止.")));
    }
    let ip = client_ip(ctx.headers);
    if ctx.state.db.spam_from_ip(&ip).await? {
        return Ok(Step::Done(plain(StatusCode::NOT_FOUND, "找不到内容")));
    }
    let author = trunc_chars(ctx.form.get("blog_name").map(|s| s.trim()).unwrap_or(""), 150);
    let url = trunc_chars(ctx.form.get("url").map(|s| s.trim()).unwrap_or(""), 255);
    let mut text = ctx.form.get("excerpt").cloned().unwrap_or_default();
    let mut message = String::new();
    if url.is_empty() {
        message = "We require all Trackbacks to provide an url.".into();
    } else if !looks_like_url(&url) {
        message = "Your url is not valid.".into();
    } else if text.trim().is_empty() {
        message = "We require all Trackbacks to provide an excerption.".into();
    } else if author.is_empty() {
        message = "We require all Trackbacks to provide an blog name.".into();
    } else if !xss_ok(&author) {
        message = "Your blog name is not valid.".into();
    }
    if !message.is_empty() {
        return Ok(Step::Done(trackback_xml(1, &message)));
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > 100 {
        text = format!("{}[...]", chars.into_iter().take(100).collect::<String>());
    }
    if ctx.state.db.ping_exists(post.cid, &url).await? {
        return Ok(Step::Done(plain(StatusCode::FORBIDDEN, "禁止重复提交")));
    }
    let status = if ctx.site.flag("commentsRequireModeration", false) {
        "waiting"
    } else {
        "approved"
    };
    let agent = trunc_chars(
        ctx.headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        511,
    );
    ctx.state
        .db
        .insert_feedback(&NewComment {
            cid: post.cid,
            created: ctx.now,
            author,
            author_id: 0,
            owner_id: post.author_id,
            mail: String::new(),
            url,
            ip: trunc_bytes(&ip, 64),
            agent,
            text,
            status: status.into(),
            kind: "trackback".into(),
            parent: 0,
        })
        .await?;
    Ok(Step::Done(trackback_xml(0, "Trackback has registered.")))
}

fn trackback_xml(success: i32, message: &str) -> Response {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><success>{success}</success><message>{}</message></response>",
        text::html_escape(message)
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/xml; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_else(|_| plain(StatusCode::OK, message))
}

fn header_nonempty(headers: &HeaderMap, name: header::HeaderName) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.trim().is_empty())
}

async fn post_comment(ctx: &ReqCtx<'_>, path: &str) -> anyhow::Result<Step> {
    let Some(post) = find_single(ctx, path).await? else {
        return Ok(Step::Next);
    };
    if !post.allow_comment
        || !comments_open(ctx.site, &post, ctx.now)
        || !unlocked(ctx, &post)
    {
        return comment_fail(
            ctx,
            &post,
            "对不起,此内容的反馈被禁止.",
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    if ctx
        .form
        .get("tiebidli_hp")
        .is_some_and(|s| !s.trim().is_empty())
    {
        return Ok(Step::Done(redirect(
            &links::content_path(ctx.site, &post),
            &HeaderMap::new(),
        )));
    }
    let viewer = auth::session_user(&ctx.state.db, &root_of(ctx), ctx.cookies)
        .await
        .ok()
        .flatten();
    let author = if let Some(user) = &viewer {
        trunc_chars(&user.label(), 150)
    } else {
        trunc_chars(ctx.form.get("author").map(|s| s.trim()).unwrap_or(""), 150)
    };
    let mail = if let Some(user) = &viewer {
        trunc_chars(&user.mail, 150)
    } else {
        trunc_chars(ctx.form.get("mail").map(|s| s.trim()).unwrap_or(""), 150)
    };
    let mut url = if let Some(user) = &viewer {
        user.url.clone()
    } else {
        ctx.form
            .get("url")
            .map(|s| s.trim())
            .unwrap_or("")
            .to_string()
    };
    if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("http://{url}");
    }
    url = trunc_chars(&url, 255);
    let text = ctx
        .form
        .get("text")
        .map(|s| s.trim())
        .unwrap_or("")
        .to_string();
    let mut fail: Option<(&str, StatusCode)> = None;
    if author.is_empty() {
        fail = Some(("必须填写用户名", StatusCode::BAD_REQUEST));
    } else if !xss_ok(&author) {
        fail = Some(("请不要在用户名中使用特殊字符", StatusCode::BAD_REQUEST));
    } else if viewer.is_none() && ctx.state.db.user_name_taken(&author).await? {
        fail = Some((
            "您所使用的用户名已经被注册,请登录后再次提交",
            StatusCode::BAD_REQUEST,
        ));
    } else if ctx.site.flag("commentsRequireMail", true) && mail.is_empty() {
        fail = Some(("必须填写电子邮箱地址", StatusCode::BAD_REQUEST));
    } else if !mail.is_empty() && !looks_like_email(&mail) {
        fail = Some(("邮箱地址不合法", StatusCode::BAD_REQUEST));
    } else if ctx.site.flag("commentsRequireURL", false) && url.is_empty() {
        fail = Some(("必须填写个人主页", StatusCode::BAD_REQUEST));
    } else if !url.is_empty() && !looks_like_url(&url) {
        fail = Some(("个人主页地址格式错误", StatusCode::BAD_REQUEST));
    } else if text.is_empty() {
        fail = Some(("必须填写评论内容", StatusCode::BAD_REQUEST));
    }
    let parent: i64 = ctx
        .form
        .get("parent")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if fail.is_none() && parent != 0 {
        let threaded = ctx.site.flag("commentsThreaded", true);
        let row = ctx.state.db.comment_parent(parent).await?;
        if !threaded || row.is_none_or(|(_, cid)| cid != post.cid) {
            fail = Some(("父级评论不存在", StatusCode::BAD_REQUEST));
        }
    }
    if fail.is_none() && ctx.site.flag("commentsCheckReferer", true) {
        let referer = ctx
            .headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let permalink = links::content_path(ctx.site, &post);
        let front = ctx.site.get("frontPage") == format!("page:{}", post.cid);
        if !referer_ok(referer, &permalink, &base_of(ctx), front) {
            fail = Some(("评论来源页错误.", StatusCode::FORBIDDEN));
        }
    }
    let ip = client_ip(ctx.headers);
    if fail.is_none()
        && ctx.site.flag("commentsPostIntervalEnable", true)
        && viewer
            .as_ref()
            .is_none_or(|user| !user.pass("editor") && user.uid != post.author_id)
    {
        let interval = ctx.site.int("commentsPostInterval", 60);
        if interval > 0 {
            let latest = ctx.state.db.latest_comment_at(post.cid, &ip).await?;
            if latest > 0 && ctx.now > latest && ctx.now - latest < interval {
                fail = Some((
                    "对不起, 您的发言过于频繁, 请稍侯再次发布.",
                    StatusCode::FORBIDDEN,
                ));
            }
        }
    }
    if let Some((msg, status)) = fail {
        return comment_fail_fields(ctx, &post, msg, status, &author, &mail, &url, &text).await;
    }
    let mut status = if ctx.site.flag("commentsRequireModeration", false) {
        "waiting"
    } else {
        "approved"
    };
    if status == "approved" && ctx.site.flag("commentsWhitelist", false) {
        let prior = ctx.state.db.approved_by_author(&author, &mail).await?;
        if prior == 0 {
            status = "waiting";
        }
    }
    if ip_blocked(ctx.site, &ip) || stop_word(ctx.site, &text) {
        status = "spam";
    }
    let agent = trunc_chars(
        ctx.headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        511,
    );
    let id = ctx
        .state
        .db
        .insert_comment(&NewComment {
            cid: post.cid,
            created: ctx.now,
            author: author.clone(),
            author_id: viewer.as_ref().map(|u| u.uid).unwrap_or(0),
            owner_id: post.author_id,
            mail: mail.clone(),
            url: url.clone(),
            ip: trunc_bytes(&ip, 64),
            agent,
            text,
            status: status.into(),
            kind: "comment".into(),
            parent,
        })
        .await?;
    let mut headers = HeaderMap::new();
    if viewer.is_none() {
        remember_cookies(&mut headers, &root_of(ctx), &author, &mail, &url, None);
    }
    let permalink = links::content_path(ctx.site, &post);
    let dest = if status == "spam" {
        permalink
    } else {
        format!("{permalink}#comment-{id}")
    };
    Ok(Step::Done(redirect(&dest, &headers)))
}

async fn comment_fail(
    ctx: &ReqCtx<'_>,
    post: &Post,
    msg: &str,
    status: StatusCode,
) -> anyhow::Result<Step> {
    comment_fail_fields(ctx, post, msg, status, "", "", "", "").await
}

async fn comment_fail_fields(
    ctx: &ReqCtx<'_>,
    post: &Post,
    msg: &str,
    status: StatusCode,
    author: &str,
    mail: &str,
    url: &str,
    text: &str,
) -> anyhow::Result<Step> {
    let mut headers = HeaderMap::new();
    if !author.is_empty() || !text.is_empty() {
        remember_cookies(&mut headers, &root_of(ctx), author, mail, url, Some(text));
    }
    show_single(
        ctx,
        post,
        SingleExtra {
            password_error: "",
            comment_error: msg,
            cookies: None,
            remember_author: nonempty_ref(author),
            remember_mail: nonempty_ref(mail),
            remember_url: nonempty_ref(url),
            remember_text: nonempty_ref(text),
            status: Some(status),
            extra: headers,
        },
    )
    .await
}

fn nonempty_ref(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn comments_open(site: &Site, post: &Post, now: i64) -> bool {
    if site.flag("commentsAutoClose", false) {
        let timeout = site.int("commentsPostTimeout", 24 * 3600 * 30);
        if timeout > 0 && now - post.created > timeout {
            return false;
        }
    }
    true
}

fn unlocked(ctx: &ReqCtx<'_>, post: &Post) -> bool {
    !post.has_password()
        || ctx
            .cookies
            .get(&protect_key(ctx, post.cid))
            .is_some_and(|v| v == &post.password)
}

fn stop_word(site: &Site, text: &str) -> bool {
    site.get("commentsStopWords").lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && text.contains(line)
    })
}

fn ip_blocked(site: &Site, ip: &str) -> bool {
    site.get("commentsIpBlackList").lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && line == ip
    })
}

fn referer_ok(referer: &str, permalink_path: &str, site_base: &str, is_front: bool) -> bool {
    let Ok(referer) = url::Url::parse(referer) else {
        return false;
    };
    let Ok(base) = url::Url::parse(site_base) else {
        return false;
    };
    if referer.host_str() != base.host_str() {
        return false;
    }
    let rp = referer.path().trim_end_matches('/');
    let pp = permalink_path.trim_end_matches('/');
    if rp.starts_with(pp) {
        return true;
    }
    is_front && rp.is_empty()
}

fn looks_like_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
}

fn looks_like_url(s: &str) -> bool {
    matches!(url::Url::parse(s), Ok(u) if u.scheme() == "http" || u.scheme() == "https")
        && !s.chars().any(char::is_whitespace)
}

fn xss_ok(s: &str) -> bool {
    !s.chars().any(|c| {
        matches!(c, '(' | ')' | '\\' | '"' | '<' | '>' | '\r' | '\n' | '\t') || (c as u32) < 0x20
    })
}

fn client_ip(headers: &HeaderMap) -> String {
    if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        let ip = v.split(',').next().unwrap_or("").trim();
        if !ip.is_empty() {
            return trunc_chars(ip, 64);
        }
    }
    if let Some(v) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if !v.trim().is_empty() {
            return trunc_chars(v.trim(), 64);
        }
    }
    "0.0.0.0".into()
}

fn remember_cookies(
    headers: &mut HeaderMap,
    root: &str,
    author: &str,
    mail: &str,
    url: &str,
    text: Option<&str>,
) {
    let month = 30 * 24 * 3600;
    push_cookie(
        headers,
        &auth::cookie_name(root, "__typecho_remember_author"),
        author,
        Some(month),
    );
    push_cookie(
        headers,
        &auth::cookie_name(root, "__typecho_remember_mail"),
        mail,
        Some(month),
    );
    push_cookie(
        headers,
        &auth::cookie_name(root, "__typecho_remember_url"),
        url,
        Some(month),
    );
    match text {
        Some(text) => push_cookie(
            headers,
            &auth::cookie_name(root, "__typecho_remember_text"),
            text,
            None,
        ),
        None => push_cookie(
            headers,
            &auth::cookie_name(root, "__typecho_remember_text"),
            "",
            Some(0),
        ),
    }
}

fn root_of(ctx: &ReqCtx<'_>) -> String {
    auth::site_root(&ctx.state.cfg.site_url, &base_of(ctx))
}

fn protect_key(ctx: &ReqCtx<'_>, cid: i64) -> String {
    auth::cookie_name(&root_of(ctx), &format!("protectPassword_{cid}"))
}

fn remember_key(ctx: &ReqCtx<'_>, key: &str) -> String {
    auth::cookie_name(&root_of(ctx), key)
}

fn split_feed(raw: &str) -> (FeedKind, String) {
    if raw == "/rss" || raw.starts_with("/rss/") {
        (FeedKind::Rdf, raw[4..].to_string())
    } else if raw == "/atom" || raw.starts_with("/atom/") {
        (FeedKind::Atom, raw[5..].to_string())
    } else {
        (FeedKind::Rss, raw.to_string())
    }
}

fn chrome<'a>(ctx: &ReqCtx<'a>, cookies: HashMap<String, String>) -> Chrome<'a> {
    Chrome {
        cfg: &ctx.state.cfg,
        site: ctx.site,
        theme_ready: crate::themes::style_ready(&ctx.state.cfg, &ctx.site.theme_name()),
        recent_posts: &ctx.bag.recent_posts,
        archives: &ctx.bag.archives,
        recent: &ctx.bag.recent,
        hot_posts: &ctx.bag.hot_posts,
        friend_links: &ctx.bag.friend_links,
        now: ctx.now,
        cookies,
        viewer_name: ctx.viewer_name,
        viewer_uid: ctx.viewer_uid,
        viewer_auth: ctx.viewer_auth,
        page_url: ctx.page_url,
        access_ids: (None, None),
        viewer_is_editor: ctx.viewer_is_editor,
    }
}

/// 后端写入模式的归档访问日志（对齐 `Plugin::backend` + `writeLogs` 的 `isAdmin` 跳过）。
/// 调用方按 `parseArchive` 语义传入 `cid`/`mid`；前端模式（`writeType=1`）靠打点，不双写。
/// 入库列对齐 `Access_Core::writeLogs`（UA 解析 + entrypoint）；
/// 外链首次访问返回 `Some((cookie名, entrypoint))`，调用方负责把 Set-Cookie 带回响应。
async fn log_archive_visit(ctx: &ReqCtx<'_>, cid: Option<i64>, mid: Option<i64>) -> Option<(String, String)> {
    if *ctx.method != Method::GET {
        return None;
    }
    if !crate::plugins::is_active(ctx.site, "Access") || ctx.viewer_is_admin {
        return None;
    }
    if crate::plugins::plugin_setting(ctx.site, "Access", "writeType", "0").trim() == "1" {
        return None;
    }
    let ua = ctx
        .headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let referer = ctx
        .headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let root = root_of(ctx);
    let ep_name = crate::auth::cookie_name(&root, crate::access::ENTRYPOINT_COOKIE);
    let cookie_ep = ctx.cookies.get(&ep_name).map(String::as_str).unwrap_or("");
    let (entrypoint, needs_set) =
        crate::access::resolve_entrypoint(&base_of(ctx), referer, cookie_ep);
    let _ = crate::access::log_visit(
        &ctx.state.db,
        ua,
        ctx.page_url,
        &client_ip(ctx.headers),
        referer,
        &entrypoint,
        ctx.now,
        cid,
        mid,
    )
    .await;
    needs_set.then(|| (ep_name, entrypoint))
}

/// `log_archive_visit` 带回的 entrypoint cookie 写入响应头。
fn attach_entrypoint(headers: &mut HeaderMap, set: Option<(String, String)>) {
    if let Some((name, value)) = set {
        push_cookie(headers, &name, &value, None);
    }
}

async fn not_found(ctx: &ReqCtx<'_>) -> anyhow::Result<Response> {
    // 404 归档同样记（`parseArchive` 明确处理 404 分支）。
    let set_ep = log_archive_visit(ctx, None, None).await;
    let chrome = chrome(ctx, ctx.cookies.clone());
    let html = ctx.state.theme.render_404(&chrome)?;
    let mut headers = HeaderMap::new();
    attach_entrypoint(&mut headers, set_ep);
    Ok(html_response(ctx, StatusCode::NOT_FOUND, html, &headers))
}

fn absolute_url(headers: &HeaderMap, path: &str, site: &Site) -> String {
    let path = if path.is_empty() { "/" } else { path };
    if let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()) {
        let https = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("https"));
        let scheme = if https { "https" } else { "http" };
        return format!("{scheme}://{host}{path}");
    }
    let base = links::site_base(&None, site);
    format!("{base}{path}")
}

fn base_of(ctx: &ReqCtx<'_>) -> String {
    links::site_base(&ctx.state.cfg.site_url, ctx.site)
}

fn page_of(params: &HashMap<String, String>) -> i64 {
    params
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1)
}

fn leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else if path.is_empty() {
        "/".into()
    } else {
        format!("/{path}")
    }
}

fn archives_slug(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/archives/")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    let rest = rest.strip_suffix(".html").unwrap_or(rest);
    if rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(rest.to_string())
}

pub fn request_path(raw: &str) -> String {
    let raw = raw.split('?').next().unwrap_or(raw);
    let decoded = decode_path(raw);
    if decoded.is_empty() {
        "/".into()
    } else if decoded.starts_with('/') {
        decoded
    } else {
        format!("/{decoded}")
    }
}

pub fn normalize_path(raw: &str) -> String {
    let raw = raw.split('?').next().unwrap_or(raw);
    let path = raw
        .strip_prefix("/index.php")
        .map(|rest| if rest.is_empty() { "/" } else { rest })
        .unwrap_or(raw);
    let decoded = path
        .split('/')
        .map(|seg| {
            urlencoding::decode(seg)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| seg.to_string())
        })
        .collect::<Vec<_>>()
        .join("/");
    let decoded = if decoded.len() > 1 {
        decoded.trim_end_matches('/').to_string()
    } else {
        decoded
    };
    if decoded.is_empty() {
        "/".into()
    } else {
        decoded
    }
}

fn form_pairs(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = decode_component(k);
        if k.is_empty() {
            continue;
        }
        map.insert(k, decode_component(v));
    }
    map
}

fn decode_component(s: &str) -> String {
    let spaced = s.replace('+', " ");
    urlencoding::decode(&spaced)
        .map(|c| c.into_owned())
        .unwrap_or(spaced)
}

fn cookies_from(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for val in headers.get_all(header::COOKIE) {
        let Ok(s) = val.to_str() else {
            continue;
        };
        for part in s.split(';') {
            let Some((k, v)) = part.trim().split_once('=') else {
                continue;
            };
            map.insert(k.to_string(), decode_component(v));
        }
    }
    map
}

fn push_cookie(headers: &mut HeaderMap, name: &str, value: &str, max_age: Option<i64>) {
    if let Some(v) = cookie_header_value(name, value, max_age) {
        headers.append(header::SET_COOKIE, v);
    }
}

fn cookie_header_value(name: &str, value: &str, max_age: Option<i64>) -> Option<HeaderValue> {
    let enc = urlencoding::encode(value);
    let mut line = format!("{name}={enc}; Path=/; SameSite=Lax");
    if let Some(age) = max_age {
        line.push_str(&format!("; Max-Age={age}"));
    }
    HeaderValue::from_str(&line).ok()
}

fn html_response(ctx: &ReqCtx<'_>, status: StatusCode, body: String, extra: &HeaderMap) -> Response {
    let mut res = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面生成失败"));
    if ctx.site.int("allowXmlRpc", 2) >= 2 {
        let url = links::abs(&base_of(ctx), &links::action_url(ctx.site, "/action/xmlrpc", false));
        if let Ok(v) = HeaderValue::from_str(&url) {
            res.headers_mut().insert("x-pingback", v);
        }
    }
    for (k, v) in extra.iter() {
        res.headers_mut().append(k, v.clone());
    }
    res
}

fn xml_response(kind: FeedKind, body: String) -> Response {
    xml_response_with(kind, body, &HeaderMap::new())
}

fn xml_response_with(kind: FeedKind, body: String, extra: &HeaderMap) -> Response {
    let ct = match kind {
        FeedKind::Rss => "application/rss+xml; charset=utf-8",
        FeedKind::Atom => "application/atom+xml; charset=utf-8",
        FeedKind::Rdf => "application/rdf+xml; charset=utf-8",
    };
    let mut res = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "订阅生成失败"));
    for (k, v) in extra.iter() {
        res.headers_mut().append(k, v.clone());
    }
    res
}

fn redirect_permanent(loc: &str) -> Response {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(header::LOCATION, loc)
        .body(Body::from("redirect"))
        .unwrap_or_else(|_| plain(StatusCode::MOVED_PERMANENTLY, loc))
}

fn redirect(loc: &str, extra: &HeaderMap) -> Response {
    let mut res = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, loc)
        .body(Body::from("redirect"))
        .unwrap_or_else(|_| plain(StatusCode::FOUND, loc));
    for (k, v) in extra.iter() {
        res.headers_mut().append(k, v.clone());
    }
    res
}

fn plain(status: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(msg.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from(msg.to_string())))
}

fn trunc_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `..*ctx` needs ReqCtx to be Copy. It contains references only.
impl<'a> Copy for ReqCtx<'a> {}
impl<'a> Clone for ReqCtx<'a> {
    fn clone(&self) -> Self {
        *self
    }
}
