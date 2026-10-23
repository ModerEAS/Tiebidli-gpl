//! 后台页面按 Typecho 1.2.1 的 `admin/` 布局和操作来。
//! 样式来自同版本的 `admin/css` 与 `admin/img`。注册只在 `allowRegister` 打开时可用。

use crate::auth::{self, SessionUser};
use crate::backup;
use crate::http::AppState;
use crate::links;
use crate::plugins;
use crate::model::{FieldValue, Post, Site};
use crate::phpval::{self, PhpValue};
use crate::render::{Chrome, SingleView};
use crate::repo::ListFilter;
use crate::text::{self, html_escape};
use crate::write::{ContentInput, FieldInput, FieldRow, MetaInput, UserInput};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{any, get, post};
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;

const PAGE: usize = 20;

tokio::task_local! {
    static PAGE_URL: String;
}

pub fn routes() -> Router<Arc<AppState>> {
    let backup_routes = Router::new()
        .route("/action/backup", any(backup_action))
        .route("/index.php/action/backup", any(backup_action))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));
    Router::new()
        .merge(backup_routes)
        .route("/action/upload", post(media_upload))
        .route("/index.php/action/upload", post(media_upload))
        .route("/action/:action", any(action_dispatch))
        .route("/index.php/action/:action", any(action_dispatch))
        .route("/admin/index.php", get(dashboard))
        .route("/admin/", get(dashboard))
        .route("/admin", get(dashboard))
        .route("/admin/login.php", get(login_form))
        .route("/admin/register.php", get(register_form))
        .route("/admin/write-post.php", get(write_post_page))
        .route("/admin/manage-posts.php", get(content_list))
        .route("/admin/write-page.php", get(write_page_page))
        .route("/admin/manage-pages.php", get(page_list))
        .route("/admin/manage-comments.php", get(comments))
        .route("/admin/manage-categories.php", get(categories))
        .route("/admin/category.php", get(category_php))
        .route("/admin/manage-tags.php", get(tags))
        .route("/admin/manage-medias.php", get(media))
        .route("/admin/media.php", get(media_php))
        .route("/admin/manage-users.php", get(users))
        .route("/admin/user.php", get(user_php))
        .route("/admin/profile.php", get(profile_page))
        .route("/admin/options-general.php", get(settings_general))
        .route("/admin/options-discussion.php", get(settings_discussion))
        .route("/admin/options-reading.php", get(settings_reading))
        .route("/admin/options-permalink.php", get(settings_permalink))
        .route("/admin/plugins.php", get(plugins_page))
        .route("/admin/options-plugin.php", get(options_plugin_page))
        .route("/admin/themes.php", get(themes_page))
        .route("/admin/options-theme.php", get(options_theme_page))
        .route("/admin/theme-editor.php", get(theme_editor_page))
        .route("/admin/backup.php", get(backup_page))
        .route("/admin/upgrade.php", get(upgrade_page))
        .route("/admin/preview", post(write_preview))
        .route("/admin/login", any(legacy_admin))
        .route("/admin/logout", any(legacy_admin))
        .route("/admin/register", any(legacy_admin))
        .route("/admin/posts", any(legacy_admin))
        .route("/admin/posts/new", any(legacy_admin))
        .route("/admin/posts/:cid", any(legacy_admin))
        .route("/admin/pages", any(legacy_admin))
        .route("/admin/pages/new", any(legacy_admin))
        .route("/admin/pages/:cid", any(legacy_admin))
        .route("/admin/categories", any(legacy_admin))
        .route("/admin/categories/new", any(legacy_admin))
        .route("/admin/categories/:mid", any(legacy_admin))
        .route("/admin/tags", any(legacy_admin))
        .route("/admin/media", any(legacy_admin))
        .route("/admin/media/:cid", any(legacy_admin))
        .route("/admin/users", any(legacy_admin))
        .route("/admin/profile", any(legacy_admin))
        .route("/admin/settings", any(legacy_admin))
        .route("/admin/settings/general", any(legacy_admin))
        .route("/admin/settings/discussion", any(legacy_admin))
        .route("/admin/settings/reading", any(legacy_admin))
        .route("/admin/settings/permalink", any(legacy_admin))
        .layer(from_fn(remember_page))
}

async fn remember_page(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("test.local");
    let url = format!("http://{host}{pq}");
    PAGE_URL.scope(url, next.run(req)).await
}

fn page_url() -> String {
    PAGE_URL.try_with(|s| s.clone()).unwrap_or_else(|_| "http://test.local/admin/index.php".into())
}

fn redirect_login() -> Response {
    let current = page_url();
    let referer = urlencoding::encode(&current);
    redirect(&format!("/admin/login.php?referer={referer}"))
}

async fn legacy_admin(uri: axum::http::Uri) -> Response {
    let dest = map_legacy(uri.path(), uri.query().unwrap_or(""));
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(header::LOCATION, dest)
        .body(Body::empty())
        .unwrap_or_else(|_| redirect("/admin/index.php"))
}

fn map_legacy(path: &str, query: &str) -> String {
    let dest = if let Some(cid) = path.strip_prefix("/admin/posts/").and_then(|s| s.trim_end_matches('/').parse::<i64>().ok()) {
        format!("/admin/write-post.php?cid={cid}")
    } else if path == "/admin/posts/new" {
        "/admin/write-post.php".into()
    } else if path == "/admin/posts" {
        "/admin/manage-posts.php".into()
    } else if let Some(cid) = path.strip_prefix("/admin/pages/").and_then(|s| s.trim_end_matches('/').parse::<i64>().ok()) {
        format!("/admin/write-page.php?cid={cid}")
    } else if path == "/admin/pages/new" {
        "/admin/write-page.php".into()
    } else if path == "/admin/pages" {
        "/admin/manage-pages.php".into()
    } else if path == "/admin/categories/new" {
        "/admin/category.php".into()
    } else if let Some(rest) = path.strip_prefix("/admin/categories/") {
        let mid = rest.split('/').next().and_then(|s| s.parse::<i64>().ok());
        if let Some(mid) = mid {
            format!("/admin/category.php?mid={mid}")
        } else {
            "/admin/manage-categories.php".into()
        }
    } else if path == "/admin/categories" {
        "/admin/manage-categories.php".into()
    } else if let Some(cid) = path.strip_prefix("/admin/media/").and_then(|s| s.split('/').next().and_then(|s| s.parse::<i64>().ok())) {
        format!("/admin/media.php?cid={cid}")
    } else if path == "/admin/media" {
        "/admin/manage-medias.php".into()
    } else {
        match path {
            "/admin/login" => "/admin/login.php".into(),
            "/admin/logout" => "/index.php/action/logout".into(),
            "/admin/register" => "/admin/register.php".into(),
            "/admin/tags" => "/admin/manage-tags.php".into(),
            "/admin/comments" => "/admin/manage-comments.php".into(),
            "/admin/users" => "/admin/manage-users.php".into(),
            "/admin/profile" => "/admin/profile.php".into(),
            "/admin/settings" | "/admin/settings/general" => "/admin/options-general.php".into(),
            "/admin/settings/discussion" => "/admin/options-discussion.php".into(),
            "/admin/settings/reading" => "/admin/options-reading.php".into(),
            "/admin/settings/permalink" => "/admin/options-permalink.php".into(),
            _ => "/admin/index.php".into(),
        }
    };
    if query.is_empty() || dest.contains('?') {
        dest
    } else {
        format!("{dest}?{query}")
    }
}

async fn write_post_page(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    editor(&state, &headers, "post", positive_id(q.get("cid"))).await
}

async fn write_page_page(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    editor(&state, &headers, "page", positive_id(q.get("cid"))).await
}

async fn category_php(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let mid = q.get("mid").and_then(|s| s.parse().ok());
    category_form(&state, &headers, mid, &q, &HashMap::new()).await
}

async fn media_php(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(cid) = q.get("cid").and_then(|s| s.parse().ok()) else {
        return redirect("/admin/manage-medias.php");
    };
    media_form(&state, &headers, cid, &q, &HashMap::new()).await
}

async fn plugins_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let main = format!("{}{}", flash_html(&q), plugins::manage_page(&site));
    layout(&site, &user, "plugins", "插件管理", None, "container", &main)
}

async fn options_plugin_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let name = q.get("config").map(String::as_str).unwrap_or("");
    match plugins::config_page(&site, name) {
        Ok((title, main)) => {
            let main = format!("{}{main}", flash_html(&q));
            layout(&site, &user, "plugins", &title, None, "container", &main)
        }
        Err(msg) => {
            let main = format!(
                r#"<div class="message notice"><ul><li>{}</li></ul></div>"#,
                html_escape(msg)
            );
            layout(&site, &user, "plugins", "设置插件", None, "container", &main)
        }
    }
}

async fn plugins_edit_action(
    state: Arc<AppState>,
    headers: &HeaderMap,
    q: &HashMap<String, String>,
    body: &str,
) -> Response {
    let Ok((site, _, _)) = need(&state, headers, "administrator").await else {
        return redirect_login();
    };
    if let Some(name) = q.get("activate") {
        return match plugins::activate(&state.db, &site, name).await {
            Ok(msg) => notice_to("/admin/plugins.php", msg),
            Err(msg) => error_to("/admin/plugins.php", msg),
        };
    }
    if let Some(name) = q.get("deactivate") {
        return match plugins::deactivate(&state.db, &site, name).await {
            Ok(msg) => notice_to("/admin/plugins.php", msg),
            Err(msg) => error_to("/admin/plugins.php", msg),
        };
    }
    if let Some(name) = q.get("config") {
        let back = format!(
            "/admin/options-plugin.php?config={}",
            urlencoding::encode(name)
        );
        return match plugins::save_config(&state.db, &site, name, &pairs(body)).await {
            Ok(()) => notice_to("/admin/plugins.php", "插件设置已经保存"),
            Err(msg) => error_to(&back, msg),
        };
    }
    redirect("/admin/plugins.php")
}

async fn themes_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let main = crate::themes::themes_main(&state.cfg, &site, &flash_html(&q));
    layout(&site, &user, "themes", "网站外观", None, "container", &main)
}

async fn options_theme_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    if !crate::themes::config_available(&state.cfg, &site) {
        return error_to("/admin/themes.php", "该外观没有设置项");
    }
    match crate::themes::config_main(&site, &flash_html(&q)) {
        Ok(main) => layout(&site, &user, "themes", "设置外观", None, "container", &main),
        Err(msg) => error_to("/admin/themes.php", msg),
    }
}

async fn theme_editor_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let (title, main) = crate::themes::editor_main(
        &state.cfg,
        &site,
        q.get("file").map(|s| s.as_str()),
        &flash_html(&q),
    );
    layout(&site, &user, "themes", &title, None, "container", &main)
}

async fn backup_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let _ = std::fs::create_dir_all(&state.cfg.backup_dir);
    let main = backup::backup_page(&state.cfg.backup_dir, &flash_html(&q));
    layout(&site, &user, "backup", "备份", None, "container", &main)
}

async fn themes_edit_action(
    state: Arc<AppState>,
    headers: &HeaderMap,
    q: &HashMap<String, String>,
    body: &str,
) -> Response {
    let Ok((site, _, _)) = need(&state, headers, "administrator").await else {
        return redirect_login();
    };
    if let Some(name) = q.get("change") {
        return match crate::themes::change_theme(&state.db, &state.cfg, &site, name).await {
            Ok(()) => notice_to("/admin/themes.php", "外观已经改变"),
            Err(msg) => error_to("/admin/themes.php", msg),
        };
    }
    let form = pairs(body);
    if form.contains_key("edit") {
        let file = form.get("edit").map(|s| s.as_str()).unwrap_or("");
        let content = form.get("content").cloned().unwrap_or_default();
        let back = crate::themes::template_html_name(file)
            .map(|name| format!("/admin/theme-editor.php?file={}", urlencoding::encode(name)))
            .unwrap_or_else(|| "/admin/theme-editor.php".into());
        return match crate::themes::save_template_file(&state.cfg, file, &content) {
            Ok(saved) => notice_to(
                &format!(
                    "/admin/theme-editor.php?file={}",
                    urlencoding::encode(&saved)
                ),
                &format!("文件 {saved} 的更改已经保存"),
            ),
            Err(msg) => error_to(&back, &msg),
        };
    }
    if q.contains_key("config") {
        if !crate::themes::config_available(&state.cfg, &site) {
            return error_to("/admin/themes.php", "该外观没有设置项");
        }
        let posted = repeated(body)
            .get("sidebarBlock[]")
            .cloned()
            .unwrap_or_default();
        let logo = form.get("logoUrl").cloned().unwrap_or_default();
        return match crate::themes::save_theme_config(&state.db, &site, &logo, &posted).await {
            Ok(()) => notice_to("/admin/options-theme.php", "外观设置已经保存"),
            Err(msg) => error_to("/admin/themes.php", msg),
        };
    }
    redirect("/admin/themes.php")
}

async fn backup_action(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    request: axum::extract::Request,
) -> Response {
    let headers = request.headers().clone();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !action_allowed(&state, &headers, &q, "").await {
        return go_back(&headers);
    }
    let Ok((_, _, root)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    if content_type.starts_with("multipart/") {
        let mut multipart = match Multipart::from_request(request, &state).await {
            Ok(multipart) => multipart,
            Err(_) => return error_to("/admin/backup.php", "备份文件上传失败"),
        };
        return backup_upload(&state, &mut multipart, &root).await;
    }
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let body = String::from_utf8_lossy(&bytes);
    let form = pairs(&body);
    let verb = q
        .get("do")
        .map(|s| s.as_str())
        .or_else(|| form.get("do").map(|s| s.as_str()))
        .unwrap_or("");
    match verb {
        "export" => {
            let retain = form.get("retain").is_some_and(|value| value == "1");
            backup_export(&state, retain).await
        }
        "import" => {
            let file = form.get("file").map(|s| s.as_str()).unwrap_or("");
            if file.is_empty() {
                error_to("/admin/backup.php", "没有选择任何备份文件")
            } else {
                match backup::read_stored(&state.cfg.backup_dir, file) {
                    Ok(bytes) => finish_import(&state, &root, &bytes).await,
                    Err(err) => error_to("/admin/backup.php", &err.message()),
                }
            }
        }
        "download" => {
            let file = q
                .get("file")
                .map(|s| s.as_str())
                .or_else(|| form.get("file").map(|s| s.as_str()))
                .unwrap_or("");
            backup_download(&state, file)
        }
        "delete" => {
            let file = q
                .get("file")
                .map(|s| s.as_str())
                .or_else(|| form.get("file").map(|s| s.as_str()))
                .unwrap_or("");
            match backup::delete_stored(&state.cfg.backup_dir, file) {
                Ok(()) => notice_to("/admin/backup.php", "备份文件已经删除"),
                Err(err) => error_to("/admin/backup.php", &err.message()),
            }
        }
        _ => redirect("/admin/backup.php"),
    }
}

async fn backup_upload(state: &AppState, multipart: &mut Multipart, root: &str) -> Response {
    let mut bytes = Vec::new();
    let mut saw = false;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            saw = true;
            bytes = field.bytes().await.unwrap_or_default().to_vec();
        }
    }
    if !saw || bytes.is_empty() {
        return error_to("/admin/backup.php", "没有选择任何备份文件");
    }
    finish_import(state, root, &bytes).await
}

async fn finish_import(state: &AppState, root: &str, bytes: &[u8]) -> Response {
    let now = chrono::Utc::now().timestamp();
    match backup::import_bytes(&state.db, bytes, now).await {
        Ok(session) => {
            let mut extra = HeaderMap::new();
            if let Some(session) = session {
                push_cookie(
                    &mut extra,
                    &auth::cookie_name(root, "__typecho_uid"),
                    &session.uid.to_string(),
                    None,
                );
                push_cookie(
                    &mut extra,
                    &auth::cookie_name(root, "__typecho_authCode"),
                    &session.auth_hash,
                    None,
                );
            }
            let loc = format!("/admin/backup.php?notice={}", urlencoding::encode("数据恢复完成"));
            redirect_with(&loc, &extra)
        }
        Err(err) => error_to("/admin/backup.php", &err.message()),
    }
}

async fn backup_export(state: &AppState, retain: bool) -> Response {
    let Ok(site) = load_site(state).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    let Ok(bytes) = backup::export_bytes(&state.db).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "备份失败");
    };
    let name = backup::export_filename(&site, chrono::Utc::now().timestamp());
    if retain {
        let _ = backup::store_copy(&state.cfg.backup_dir, &name, &bytes);
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from(bytes))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "备份失败"))
}

fn backup_download(state: &AppState, file: &str) -> Response {
    match backup::read_stored(&state.cfg.backup_dir, file) {
        Ok(bytes) => {
            let name = backup::safe_dat_name(file).unwrap_or_else(|| "backup.dat".into());
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                )
                .body(Body::from(bytes))
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "无法读取备份文件"))
        }
        Err(err) => error_to("/admin/backup.php", &err.message()),
    }
}

async fn action_dispatch(
    State(state): State<Arc<AppState>>,
    Path(action): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    body: String,
) -> Response {
    if action == "xmlrpc" {
        let now = chrono::Utc::now().timestamp();
        let Ok(site) = state.db.load_site(now).await else {
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
        };
        return crate::xmlrpc::handle(&state.db, &state.cfg, &site, &method, &q, body.as_bytes(), now, &headers)
            .await
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "xmlrpc"));
    }
    if action == "ajax" {
        return ajax_action(&state, &q, &headers, &body).await;
    }
    if !action_allowed(&state, &headers, &q, &body).await {
        return go_back(&headers);
    }
    let form = pairs(&body);
    let verb = q.get("do").map(|s| s.as_str()).or_else(|| form.get("do").map(|s| s.as_str())).unwrap_or("");
    match action.as_str() {
        "login" => login_submit(State(state), headers, body).await,
        "logout" => logout(State(state), headers).await,
        "register" => register_submit(State(state), body).await,
        "contents-post-edit" => content_action(state, &headers, "post", &q, verb, &body).await,
        "contents-page-edit" => content_action(state, &headers, "page", &q, verb, &body).await,
        "comments-edit" => comments_action(state, &headers, &q, verb, &body).await,
        "metas-category-edit" => category_action(state, &headers, &q, verb, &body).await,
        "metas-tag-edit" => tag_action(state, &headers, &q, verb, &body).await,
        "contents-attachment-edit" => attachment_action(state, &headers, &q, verb, &body).await,
        "users-edit" => users_action(state, &headers, &q, verb, &body).await,
        "users-profile" => profile_save(State(state), headers, body).await,
        "options-general" => settings_save_general(State(state), headers, body).await,
        "options-discussion" => settings_save_discussion(State(state), headers, body).await,
        "options-reading" => settings_save_reading(State(state), headers, body).await,
        "options-permalink" => settings_save_permalink(State(state), headers, body).await,
        "themes-edit" => themes_edit_action(state, &headers, &q, &body).await,
        "plugins-edit" => plugins_edit_action(state, &headers, &q, &body).await,
        _ => plain(StatusCode::NOT_FOUND, "请求的地址不存在"),
    }
}

async fn ajax_action(state: &AppState, q: &HashMap<String, String>, headers: &HeaderMap, body: &str) -> Response {
    let ajax = headers
        .get("x-requested-with")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("XMLHttpRequest"));
    if !ajax {
        return go_back(headers);
    }
    let form = pairs(body);
    let verb = q.get("do").map(|s| s.as_str()).or_else(|| form.get("do").map(|s| s.as_str()));
    if verb == Some("remoteCallback") {
        let site = load_site(state).await.unwrap_or_else(|_| crate::model::site_from_parts(HashMap::new(), Vec::new(), Vec::new(), Vec::new()));
        let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
        if agent == site.get("generator") {
            return plain(StatusCode::OK, "OK");
        }
    }
    plain(StatusCode::OK, "")
}

async fn action_allowed(state: &AppState, headers: &HeaderMap, q: &HashMap<String, String>, body: &str) -> bool {
    let Ok(site) = load_site(state).await else {
        return false;
    };
    let form = pairs(body);
    let given = q.get("_").map(|s| s.as_str()).or_else(|| form.get("_").map(|s| s.as_str())).unwrap_or("");
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok()).unwrap_or("");
    let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, &site));
    let cookies = cookies_from(headers);
    let user = auth::session_user(&state.db, &root, &cookies).await.ok().flatten();
    let token = auth::security_token(site.get("secret"), user.as_ref().map(|u| (u.auth_code.as_str(), u.uid)), referer);
    given == token
}

fn go_back(headers: &HeaderMap) -> Response {
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok()).unwrap_or("");
    if referer.starts_with("http://") || referer.starts_with("https://") || referer.starts_with('/') {
        redirect(referer)
    } else {
        redirect("/")
    }
}

async fn content_action(state: Arc<AppState>, headers: &HeaderMap, kind: &str, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    let back = if kind == "page" { "/admin/manage-pages.php" } else { "/admin/manage-posts.php" };
    let form = pairs(body);
    match verb {
        "delete" | "mark" => {
            let mut query = q.clone();
            query.insert("do".into(), verb.into());
            if let Some(status) = q.get("status").cloned().or_else(|| form.get("status").cloned()) {
                query.insert("status".into(), status);
            }
            content_batch(&state, headers, kind, back, &query, body).await
        }
        "deleteDraft" => {
            let cid = positive_id(q.get("cid")).or_else(|| positive_id(form.get("cid"))).unwrap_or(0);
            delete_kind(&state, headers, cid, back).await
        }
        _ => {
            let cid = positive_id(q.get("cid")).or_else(|| positive_id(form.get("cid")));
            save_kind(&state, headers, kind, cid, body).await
        }
    }
}

async fn comments_action(state: Arc<AppState>, headers: &HeaderMap, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    match verb {
        "edit" => comment_edit(State(state), headers.clone(), Query(q.clone()), body.to_string()).await,
        "reply" => comment_reply(State(state), headers.clone(), Query(q.clone()), body.to_string()).await,
        _ => {
            if q.get("coid").is_some() && !body.contains("coid[]") {
                comment_get_act(State(state), headers.clone(), Query(q.clone())).await
            } else {
                comments_batch(State(state), headers.clone(), Query(q.clone()), body.to_string()).await
            }
        }
    }
}

async fn category_action(state: Arc<AppState>, headers: &HeaderMap, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    match verb {
        "default" => {
            let mid = q.get("mid").and_then(|s| s.parse().ok()).unwrap_or(0);
            category_default_mid(&state, headers, mid).await
        }
        "insert" | "update" | "" => category_commit(&state, headers, q.get("mid").and_then(|s| s.parse().ok()), body).await,
        _ => {
            let mut query = q.clone();
            query.entry("do".into()).or_insert_with(|| verb.to_string());
            categories_batch(State(state), headers.clone(), Query(query), body.to_string()).await
        }
    }
}

async fn tag_action(state: Arc<AppState>, headers: &HeaderMap, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    if verb == "insert" || verb == "update" || (verb.is_empty() && pairs(body).contains_key("name")) {
        meta_save(&state, headers, "tag", "/admin/manage-tags.php", body).await
    } else {
        let mut query = q.clone();
        if !verb.is_empty() {
            query.insert("do".into(), verb.into());
        }
        meta_batch(&state, headers, "/admin/manage-tags.php", &query, body).await
    }
}

async fn attachment_action(state: Arc<AppState>, headers: &HeaderMap, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    let form = pairs(body);
    let cid = q.get("cid").and_then(|s| s.parse().ok()).or_else(|| form.get("cid").and_then(|s| s.parse().ok())).unwrap_or(0);
    if verb == "update" || (form.contains_key("name") && verb != "delete" && verb != "clear") {
        media_update(State(state), headers.clone(), Path(cid), body.to_string()).await
    } else if verb == "delete" && cid > 0 && !body.contains("cid[]") {
        media_delete(State(state), headers.clone(), Path(cid), body.to_string()).await
    } else {
        media_batch(State(state), headers.clone(), Query(q.clone()), body.to_string()).await
    }
}

async fn users_action(state: Arc<AppState>, headers: &HeaderMap, q: &HashMap<String, String>, verb: &str, body: &str) -> Response {
    if verb == "delete" {
        if let Some(uid) = q.get("uid").and_then(|s| s.parse().ok()).or_else(|| pairs(body).get("uid").and_then(|s| s.parse().ok())) {
            if !body.contains("uid[]") {
                return user_delete(State(state), headers.clone(), Path(uid)).await;
            }
        }
        users_batch(State(state), headers.clone(), Query(q.clone()), body.to_string()).await
    } else {
        user_commit(&state, headers, verb, body).await
    }
}

async fn category_default_mid(state: &AppState, headers: &HeaderMap, mid: i64) -> Response {
    let Ok((_, _, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let Some(meta) = state.db.meta_by_id(mid).await.ok().flatten().filter(|m| m.kind == "category") else {
        return error_to("/admin/manage-categories.php", "分类不存在");
    };
    let _ = state.db.set_option("defaultCategory", &mid.to_string()).await;
    notice_to("/admin/manage-categories.php", &format!("{} 已经被设为默认分类", meta.name))
}

async fn dashboard(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "subscriber").await else {
        return redirect_login();
    };
    let posts = state.db.list_contents("post", 200).await.unwrap_or_default();
    let comments = state.db.list_comments(200).await.unwrap_or_default();
    let mine: Vec<&Post> = posts.iter().filter(|p| p.author_id == user.uid).collect();
    let published = mine.iter().filter(|p| p.status == "publish").count();
    let my_comments = comments.iter().filter(|c| c.owner_id == user.uid && c.status == "approved").count();
    let waiting = comments.iter().filter(|c| c.status == "waiting").count();
    let cats = site.categories.len();
    let mut start = String::new();
    if user.pass("contributor") {
        start.push_str("<li><a href=\"/admin/write-post.php\">撰写新文章</a></li>");
    }
    if waiting > 0 && user.pass("editor") {
        start.push_str(&format!(
            "<li><a href=\"/admin/manage-comments.php?status=waiting\">待审核的评论</a><span class=\"balloon\">{waiting}</span></li>"
        ));
    }
    if user.pass("administrator") {
        start.push_str("<li><a href=\"/admin/options-general.php\">系统设置</a></li>");
    }
    let mut recent_posts = String::new();
    let shown: Vec<&Post> = mine.iter().filter(|p| p.status == "publish").take(10).copied().collect();
    if shown.is_empty() {
        recent_posts.push_str("<li><em>暂时没有文章</em></li>");
    } else {
        for post in shown {
            recent_posts.push_str(&format!(
                "<li><span>{}</span> <a href=\"{}\" class=\"title\">{}</a></li>",
                date_short(post.created, site.timezone()),
                html_escape(&links::content_path(&site, post)),
                html_escape(&post.title)
            ));
        }
    }
    let mut recent_comments = String::new();
    let mine_comments: Vec<_> = comments.iter().filter(|c| c.owner_id == user.uid).take(10).collect();
    if mine_comments.is_empty() {
        recent_comments.push_str("<li>暂时没有回复</li>");
    } else {
        for c in mine_comments {
            recent_comments.push_str(&format!(
                "<li><span>{}</span> <a class=\"title\" href=\"/admin/manage-comments.php?cid={}\">{}</a>: {}</li>",
                date_short(c.created, site.timezone()),
                c.cid,
                html_escape(&c.author),
                html_escape(&crate::text::truncate_chars(&c.text, 35))
            ));
        }
    }
    let body = format!(
        r#"<div class="row typecho-page-main">
<div class="col-mb-12 welcome-board" role="main">
<p>目前有 <em>{published}</em> 篇文章, 并有 <em>{my_comments}</em> 条关于你的评论在 <em>{cats}</em> 个分类中.<br>点击下面的链接快速开始:</p>
<ul id="start-link" class="clearfix">{start}</ul>
<p class="description">当前兼容版本 <strong>1.2.1-compat</strong>。升级说明见 <a href="/admin/upgrade.php">upgrade.php</a>。</p>
</div>
<div class="col-mb-12 col-tb-4" role="complementary"><section class="latest-link"><h3>最近发布的文章</h3><ul>{recent_posts}</ul></section></div>
<div class="col-mb-12 col-tb-4" role="complementary"><section class="latest-link"><h3>最近得到的回复</h3><ul>{recent_comments}</ul></section></div>
<div class="col-mb-12 col-tb-4" role="complementary"><section class="latest-link"><h3>官方最新日志</h3><div id="typecho-message"><ul><li>不拉取外部日志</li></ul></div></section></div>
</div>"#
    );
    layout(&site, &user, "dashboard", "概要", None, "container typecho-dashboard", &body)
}

async fn upgrade_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let main = r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12"><div id="typecho-welcome"><h3>手动升级</h3><ul><li>当前兼容版本是 <strong>1.2.1-compat</strong>。</li><li>升级时替换这个程序的二进制，然后重启进程。</li><li>这里不拉取远程版本，也不执行数据库升级脚本。</li><li><strong class="warning">升级之前请先<a href="/admin/backup.php">备份您的数据</a></strong></li></ul></div></div></div>"#;
    layout(&site, &user, "dashboard", "升级", None, "container", main)
}

async fn login_form(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if session(&state, &headers).await.ok().flatten().is_some() {
        return redirect("/admin/index.php");
    }
    let Ok(site) = load_site(&state).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    let error = q.get("error").map(|s| !s.is_empty()).unwrap_or(false);
    let referer = q.get("referer").map(|s| s.as_str()).unwrap_or("");
    let register = if site.flag("allowRegister", false) {
        " &bull;\n<a href=\"/admin/register.php\">用户注册</a>"
    } else {
        ""
    };
    let message = if error {
        "<div class=\"message error\">用户名或密码无效</div>"
    } else {
        ""
    };
    let html = format!(
        r#"<div class="typecho-login-wrap"><div class="typecho-login">
<h1><a href="https://typecho.org" class="i-logo">Typecho</a></h1>
{message}
<form action="/action/login" method="post" name="login" role="form">
<p><label for="name" class="sr-only">用户名</label>
<input type="text" id="name" name="name" placeholder="用户名" class="text-l w-100" autofocus></p>
<p><label for="password" class="sr-only">密码</label>
<input type="password" id="password" name="password" class="text-l w-100" placeholder="密码"></p>
<p class="submit"><button type="submit" class="btn btn-l w-100 primary">登录</button>
<input type="hidden" name="referer" value="{referer}"></p>
<p><label for="remember"><input type="checkbox" name="remember" class="checkbox" value="1" id="remember"> 下次自动登录</label></p>
</form>
<p class="more-link"><a href="/">返回首页</a>{register}</p>
</div></div>"#,
        referer = html_escape(referer),
    );
    guest(&site, "登录", "body-100", &html)
}

async fn login_submit(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let form = pairs(&body);
    let name = form.get("name").map(|s| s.as_str()).unwrap_or("");
    let password = form.get("password").map(|s| s.as_str()).unwrap_or("");
    let base = match load_site(&state).await {
        Ok(site) => links::site_base(&state.cfg.site_url, &site),
        Err(_) => String::new(),
    };
    let next = safe_next(form.get("referer").map(|s| s.as_str()).unwrap_or(""), &base);
    let Ok(Some(user)) = auth::login_user(&state.db, name, password).await else {
        return redirect("/admin/login.php?error=1");
    };
    let Ok(code) = auth::commit_login(&state.db, user.uid).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "登录失败");
    };
    let root = root_of(&state, &headers).await;
    let age = if form.contains_key("remember") { Some(30 * 24 * 3600) } else { None };
    let mut extra = HeaderMap::new();
    push_cookie(&mut extra, &auth::cookie_name(&root, "__typecho_uid"), &user.uid.to_string(), age);
    push_cookie(&mut extra, &auth::cookie_name(&root, "__typecho_authCode"), &code, age);
    redirect_with(&next, &extra)
}

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let root = root_of(&state, &headers).await;
    if let Ok(Some(user)) = session(&state, &headers).await {
        let sql = format!("UPDATE {} SET authCode = NULL WHERE uid = ?", state.db.table("users"));
        let _ = sqlx::query(&sql).bind(user.uid).execute(&state.db.pool).await;
    }
    let mut extra = HeaderMap::new();
    push_cookie(&mut extra, &auth::cookie_name(&root, "__typecho_uid"), "", Some(0));
    push_cookie(&mut extra, &auth::cookie_name(&root, "__typecho_authCode"), "", Some(0));
    redirect_with("/admin/login.php", &extra)
}

async fn register_form(State(state): State<Arc<AppState>>, _headers: HeaderMap) -> Response {
    let Ok(site) = load_site(&state).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    if !site.flag("allowRegister", false) {
        return plain(StatusCode::NOT_FOUND, "未开放注册");
    }
    guest(
        &site,
        "注册",
        "body-100",
        r#"<div class="typecho-login-wrap"><div class="typecho-login">
<h1><a href="https://typecho.org" class="i-logo">Typecho</a></h1>
<form method="post" action="/action/register" role="form">
<p><input class="text-l w-100" name="name" placeholder="用户名" required></p>
<p><input class="text-l w-100" name="mail" type="email" placeholder="邮箱" required></p>
<p><input class="text-l w-100" name="password" type="password" placeholder="密码" required></p>
<p class="submit"><button type="submit" class="btn btn-l w-100 primary">注册</button></p>
</form>
<p class="more-link"><a href="/admin/login.php">返回登录</a></p>
</div></div>"#,
    )
}

async fn register_submit(State(state): State<Arc<AppState>>, body: String) -> Response {
    let Ok(site) = load_site(&state).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取设置失败");
    };
    if !site.flag("allowRegister", false) {
        return plain(StatusCode::NOT_FOUND, "未开放注册");
    }
    let form = pairs(&body);
    let now = chrono::Utc::now().timestamp();
    match state
        .db
        .save_user(&UserInput {
            uid: None,
            name: form.get("name").cloned().unwrap_or_default(),
            password: form.get("password").cloned().unwrap_or_default(),
            mail: form.get("mail").cloned().unwrap_or_default(),
            url: String::new(),
            screen: form.get("name").cloned().unwrap_or_default(),
            group: "subscriber".into(),
            created: now,
        })
        .await
    {
        Ok(_) => guest(
            &site,
            "注册",
            "body-100",
            r#"<div class="typecho-login-wrap"><div class="typecho-login"><p class="message success">注册成功，请<a href="/admin/login.php">登录</a></p></div></div>"#,
        ),
        Err(_) => guest(
            &site,
            "注册",
            "body-100",
            r#"<div class="typecho-login-wrap"><div class="typecho-login"><p class="message error">注册失败，用户名或邮箱可能已存在</p><p class="more-link"><a href="/admin/register.php">返回</a></p></div></div>"#,
        ),
    }
}

async fn content_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    list_kind(&state, &headers, "post", &q).await
}
async fn page_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    list_kind(&state, &headers, "page", &q).await
}

async fn list_kind(state: &AppState, headers: &HeaderMap, kind: &str, q: &HashMap<String, String>) -> Response {
    let need_rank = if kind == "page" { "editor" } else { "contributor" };
    let Ok((site, user, _)) = need(state, headers, need_rank).await else {
        return redirect_login();
    };
    let Ok(rows) = state.db.list_manage_contents(kind, 500).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "读取失败");
    };
    let status = q.get("status").map(|s| s.as_str()).unwrap_or("");
    let keywords = q.get("keywords").map(|s| s.as_str()).unwrap_or("");
    let category = q.get("category").and_then(|s| s.parse::<i64>().ok());
    let uid = q.get("uid").and_then(|s| s.parse::<i64>().ok());
    let all = q.get("__typecho_all_posts").map(|s| s == "on").unwrap_or(false);
    let filtered: Vec<&Post> = rows
        .iter()
        .filter(|p| status_tab(status, p))
        .filter(|p| author_visible(&user, all, uid, p.author_id))
        .filter(|p| category.is_none_or(|mid| p.categories.iter().any(|c| c.mid == mid)))
        .filter(|p| keywords.is_empty() || title_hit(&p.title, keywords))
        .collect();
    let page = q.get("page").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1);
    let pages = filtered.len().div_ceil(PAGE).max(1);
    let page = page.min(pages);
    let slice = &filtered[(page - 1) * PAGE..filtered.len().min(page * PAGE)];
    let base = if kind == "page" { "/admin/manage-pages.php" } else { "/admin/manage-posts.php" };
    let write = if kind == "page" { "/admin/write-page.php" } else { "/admin/write-post.php" };
    let edit = if kind == "page" { "/admin/write-page.php?cid=" } else { "/admin/write-post.php?cid=" };
    let batch = if kind == "page" { "/action/contents-page-edit" } else { "/action/contents-post-edit" };
    let title = if kind == "page" { "管理独立页面" } else { "管理文章" };
    let key = if kind == "page" { "pages" } else { "posts" };
    let waiting = rows.iter().filter(|p| status_tab("waiting", p) && author_visible(&user, all, uid, p.author_id)).count();
    let drafts = rows.iter().filter(|p| status_tab("draft", p) && author_visible(&user, all, uid, p.author_id)).count();
    let mut tabs = String::new();
    for (name, label, n) in [("" , "可用", 0usize), ("waiting", "待审核", waiting), ("draft", "草稿", drafts)] {
        if kind == "page" && name == "waiting" {
            continue;
        }
        let current = if status == name { " class=\"current\"" } else { "" };
        let balloon = if n > 0 { format!(" <span class=\"balloon\">{n}</span>") } else { String::new() };
        tabs.push_str(&format!(
            "<li{current}><a href=\"{base}?status={name}\">{label}{balloon}</a></li>"
        ));
    }
    let mut scope = String::new();
    if user.pass("editor") && kind == "post" {
        let mine = if all { "" } else { " class=\"current\"" };
        let every = if all { " class=\"current\"" } else { "" };
        scope = format!(
            "<ul class=\"typecho-option-tabs right\"><li{every}><a href=\"{base}?__typecho_all_posts=on&status={status}\">所有</a></li><li{mine}><a href=\"{base}?__typecho_all_posts=off&status={status}\">我的</a></li></ul>"
        );
    }
    let mut marks = String::new();
    if user.pass("editor") {
        for (st, label) in [("publish", "公开"), ("waiting", "待审核"), ("hidden", "隐藏"), ("private", "私密")] {
            marks.push_str(&format!(
                "<li><a href=\"{batch}?do=mark&status={st}\">标记为<strong>{label}</strong></a></li>"
            ));
        }
    }
    let mut cats = String::from("<option value=\"\">所有分类</option>");
    if kind == "post" {
        for c in &site.categories {
            let sel = if category == Some(c.mid) { " selected" } else { "" };
            cats.push_str(&format!("<option value=\"{}\"{sel}>{}</option>", c.mid, html_escape(&c.name)));
        }
    }
    let search = if kind == "post" {
        format!(
            r#"<input type="text" class="text-s" placeholder="请输入关键字" name="keywords" value="{kw}">
<select name="category">{cats}</select>"#,
            kw = html_escape(keywords)
        )
    } else {
        format!(
            r#"<input type="text" class="text-s" placeholder="请输入关键字" name="keywords" value="{}">"#,
            html_escape(keywords)
        )
    };
    let hidden_status = if status.is_empty() {
        String::new()
    } else {
        format!("<input type=\"hidden\" name=\"status\" value=\"{status}\">")
    };
    let mut body_rows = String::new();
    if slice.is_empty() {
        let empty = if kind == "page" { "没有任何页面" } else { "没有任何文章" };
        body_rows.push_str(&format!("<tr><td colspan=\"6\"><h6 class=\"typecho-list-table-title\">{empty}</h6></td></tr>"));
    }
    for post in slice {
        let badge = status_badge(post);
        let view = links::content_path(&site, post);
        body_rows.push_str(&format!(
            r#"<tr id="post-{cid}"><td class="kit-hidden-mb"><input type="checkbox" value="{cid}" name="cid[]"></td>
<td class="kit-hidden-mb"><a href="/admin/manage-comments.php?cid={cid}" class="balloon-button" title="{n} 评论">{n}</a></td>
<td><a href="{edit}{cid}">{title}</a>{badge}
<a href="{edit}{cid}" title="编辑 {title}"><i class="i-edit"></i></a>
<a href="{view}" title="浏览 {title}"><i class="i-exlink"></i></a></td>
<td class="kit-hidden-mb"><a href="{base}?uid={uid}">{author}</a></td>
<td class="kit-hidden-mb">{cats}</td>
<td>{date}</td></tr>"#,
            cid = post.cid,
            n = post.comments_num,
            title = html_escape(&post.title),
            view = html_escape(&view),
            uid = post.author_id,
            author = html_escape(&post.display_author()),
            cats = post.categories.iter().map(|c| html_escape(&c.name)).collect::<Vec<_>>().join(", "),
            date = html_escape(&date_word(post.created, site.timezone())),
        ));
    }
    let pager = pager_html(page, pages, base, q);
    let cat_filter = search;
    let main = format!(
        r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12 typecho-list">
<div class="clearfix">{scope}<ul class="typecho-option-tabs">{tabs}</ul></div>
<div class="typecho-list-operate clearfix"><form method="get">
<div class="operate"><label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button">选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu"><li><a lang="你确认要删除这些文章吗?" href="{batch}?do=delete">删除</a></li>{marks}</ul></div></div>
<div class="search" role="search">{cat_filter}<button type="submit" class="btn btn-s">筛选</button>{hidden_status}</div>
</form></div>
<form method="post" name="manage_posts" class="operate-form" action="{batch}">
<div class="typecho-table-wrap"><table class="typecho-list-table"><colgroup>
<col width="20" class="kit-hidden-mb"><col width="6%" class="kit-hidden-mb"><col width="45%"><col width="" class="kit-hidden-mb"><col width="18%" class="kit-hidden-mb"><col width="16%">
</colgroup><thead><tr><th class="kit-hidden-mb"></th><th class="kit-hidden-mb"></th><th>标题</th><th class="kit-hidden-mb">作者</th><th class="kit-hidden-mb">分类</th><th>日期</th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form>
<div class="typecho-list-operate clearfix"><form method="get"><div class="operate"><label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label></div>{pager}</form></div>
</div></div>"#
    );
    layout(&site, &user, key, title, Some(write), "container", &main)
}

fn positive_id(raw: Option<&String>) -> Option<i64> {
    raw.and_then(|s| s.parse().ok()).filter(|n| *n > 0)
}

fn same_content_kind(stored: &str, kind: &str) -> bool {
    stored == kind || stored.strip_suffix("_draft") == Some(kind)
}

fn is_type_draft(post: &Post) -> bool {
    post.kind.ends_with("_draft")
}

fn status_tab(filter: &str, post: &Post) -> bool {
    let draft = post.status == "draft" || is_type_draft(post);
    match filter {
        "waiting" => post.status == "waiting" && !is_type_draft(post),
        "draft" => draft,
        _ => post.status != "waiting" && !draft,
    }
}

fn author_visible(user: &SessionUser, all: bool, uid: Option<i64>, author: i64) -> bool {
    if let Some(uid) = uid {
        return author == uid;
    }
    if !user.pass("editor") || !all {
        return author == user.uid;
    }
    true
}

fn title_hit(title: &str, keywords: &str) -> bool {
    let title = title.to_lowercase();
    keywords.split_whitespace().all(|w| title.contains(&w.to_lowercase()))
}

fn status_badge(post: &Post) -> String {
    let label = if post.status == "draft" || is_type_draft(post) {
        "草稿"
    } else if post.status == "hidden" {
        "隐藏"
    } else if post.status == "waiting" {
        "待审核"
    } else if post.status == "private" {
        "私密"
    } else if post.has_password() {
        "密码保护"
    } else {
        return String::new();
    };
    format!(" <em class=\"status\">{label}</em>")
}

async fn content_new(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    editor(&state, &headers, "post", None).await
}
async fn page_new(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    editor(&state, &headers, "page", None).await
}
async fn content_edit(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>) -> Response {
    editor(&state, &headers, "post", Some(cid)).await
}
async fn page_edit(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>) -> Response {
    editor(&state, &headers, "page", Some(cid)).await
}

async fn editor(state: &AppState, headers: &HeaderMap, kind: &str, cid: Option<i64>) -> Response {
    let cid = cid.filter(|n| *n > 0);
    let need_rank = if kind == "page" { "editor" } else { "contributor" };
    let Ok((site, user, _)) = need(state, headers, need_rank).await else {
        return redirect_login();
    };
    let existing = if let Some(cid) = cid {
        match state.db.content_by_id(cid).await {
            Ok(Some(post)) if same_content_kind(&post.kind, kind) && (user.pass("editor") || post.author_id == user.uid) => Some(post),
            Ok(Some(_)) => return plain(StatusCode::FORBIDDEN, "不能编辑这条内容"),
            _ => return plain(StatusCode::NOT_FOUND, "内容不存在"),
        }
    } else {
        None
    };
    let action = if kind == "page" {
        "/action/contents-page-edit"
    } else {
        "/action/contents-post-edit"
    };
    let title = existing.as_ref().map(|p| p.title.as_str()).unwrap_or("");
    let slug = existing.as_ref().map(|p| p.slug.as_str()).unwrap_or("");
    let text = existing.as_ref().map(|p| p.text.as_str()).unwrap_or("");
    let status = existing.as_ref().map(|p| p.status.as_str()).unwrap_or("");
    let password = existing.as_ref().map(|p| p.password.as_str()).unwrap_or("");
    let created = existing.as_ref().map(|p| p.created).unwrap_or(0);
    let date_text = if created > 0 {
        date_word(created, site.timezone())
    } else {
        String::new()
    };
    let visibility = if !password.is_empty() && status == "publish" {
        "password"
    } else if status.is_empty() {
        "publish"
    } else {
        status
    };
    let pattern = site
        .routes
        .routes
        .iter()
        .find(|r| r.name == if kind == "page" { "page" } else { "post" })
        .map(|r| r.source.as_str())
        .unwrap_or(if kind == "page" { "/[slug].html" } else { "/archives/[cid:digital]/" });
    let mut permalink = decode_rule(pattern);
    if let Some(id) = cid {
        permalink = permalink.replace("{cid}", &id.to_string());
    }
    let slug_input = format!(
        "<input type=\"text\" id=\"slug\" name=\"slug\" autocomplete=\"off\" value=\"{}\" class=\"mono\">",
        html_escape(slug)
    );
    let slug_html = if permalink.contains("{slug}") {
        permalink.replace("{slug}", &slug_input)
    } else {
        format!("{permalink} {slug_input}")
    };
    let draft = if status == "draft" || existing.as_ref().is_some_and(|p| is_type_draft(p)) {
        "<cite class=\"edit-draft-notice\">当前正在编辑的是未发布的草稿</cite>"
    } else {
        ""
    };
    let mut vis = String::new();
    if user.pass("editor") {
        for (value, label) in [
            ("publish", "公开"),
            ("hidden", "隐藏"),
            ("password", "密码保护"),
            ("private", "私密"),
            ("waiting", "待审核"),
        ] {
            let sel = if visibility == value { " selected" } else { "" };
            vis.push_str(&format!("<option value=\"{value}\"{sel}>{label}</option>"));
        }
    }
    let pwd_hidden = if visibility == "password" { "" } else { " class=\"hidden\"" };
    let visibility_html = if user.pass("editor") {
        format!(
            r#"<section class="typecho-post-option visibility-option"><label for="visibility" class="typecho-label">公开度</label>
<p><select id="visibility" name="visibility">{vis}</select></p>
<p id="post-password"{pwd_hidden}><label for="protect-pwd" class="sr-only">内容密码</label>
<input type="text" name="password" id="protect-pwd" class="text-s" value="{password}" placeholder="内容密码" autocomplete="off"></p></section>"#,
            password = html_escape(password)
        )
    } else {
        String::new()
    };
    let mut cat_html = String::new();
    if kind == "post" {
        let selected: Vec<i64> = existing.as_ref().map(|p| p.categories.iter().map(|c| c.mid).collect()).unwrap_or_default();
        cat_html.push_str("<section class=\"typecho-post-option category-option\"><label class=\"typecho-label\">分类</label><ul>");
        for c in &site.categories {
            let checked = if selected.contains(&c.mid) { " checked" } else { "" };
            cat_html.push_str(&format!(
                "<li><input type=\"checkbox\" id=\"category-{mid}\" value=\"{mid}\" name=\"category[]\"{checked}><label for=\"category-{mid}\">{name}</label></li>",
                mid = c.mid,
                name = html_escape(&c.name)
            ));
        }
        cat_html.push_str("</ul></section>");
    }
    let tags = existing
        .as_ref()
        .map(|p| p.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    let check = |on: bool| if on { " checked" } else { "" };
    let own = state.db.user_options(user.uid).await.unwrap_or_default();
    let allow_comment = existing
        .as_ref()
        .map(|p| p.allow_comment)
        .unwrap_or_else(|| option_flag(&own, "defaultAllowComment", true));
    let allow_ping = existing
        .as_ref()
        .map(|p| p.allow_ping)
        .unwrap_or_else(|| option_flag(&own, "defaultAllowPing", true));
    let allow_feed = existing
        .as_ref()
        .map(|p| p.allow_feed)
        .unwrap_or_else(|| option_flag(&own, "defaultAllowFeed", true));
    let was_markdown = text.starts_with("<!--markdown-->");
    let markdown_on = option_flag(&own, "markdown", false);
    let show_markdown = markdown_on && (existing.is_none() || was_markdown);
    let display_text = if show_markdown {
        text.strip_prefix("<!--markdown-->").unwrap_or(text)
    } else {
        text
    };
    let markdown_input = if show_markdown {
        r#"<input type="hidden" name="markdown" value="1">"#
    } else {
        ""
    };
    let field_rows = if let Some(post) = existing.as_ref() {
        state.db.fields_for(post.cid).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let fields_html = custom_fields_html(&field_rows);
    let preview_label = if kind == "page" { "预览页面" } else { "预览文章" };
    let publish_label = if kind == "page" { "发布页面" } else { "发布文章" };
    let form_name = if kind == "page" { "write_page" } else { "write_post" };
    let trackback_html = if kind == "post" {
        r#"<section class="typecho-post-option"><label for="trackback" class="typecho-label">引用通告</label>
<p><textarea id="trackback" class="w-100 mono" name="trackback" rows="2"></textarea></p>
<p class="description">每一行一个引用地址, 用回车隔开</p></section>"#
    } else {
        ""
    };
    let edited_note = if let Some(post) = existing.as_ref() {
        format!(
            r#"<section class="typecho-post-option"><p class="description"><br>&mdash;<br>本文由 <a href="/admin/manage-posts.php?uid={uid}">{author}</a> 撰写<br>最后更新于 {modified}</p></section>"#,
            uid = post.author_id,
            author = html_escape(&post.display_author()),
            modified = html_escape(&date_word(post.modified, site.timezone())),
        )
    } else {
        String::new()
    };
    let page_title = match (&existing, kind) {
        (Some(p), "page") => format!("编辑 {}", p.title),
        (Some(p), _) => format!("编辑 {}", p.title),
        (None, "page") => "创建新页面".into(),
        _ => "撰写新文章".into(),
    };
    let key = if kind == "page" { "write-page" } else { "write-post" };
    let parent = cid.unwrap_or(0);
    let cid_field = if parent > 0 {
        format!("<input type=\"hidden\" name=\"cid\" value=\"{parent}\">")
    } else {
        String::new()
    };
    let (files_html, files_count) = attachment_tab(state, &user, cid).await;
    let files_balloon = if files_count > 0 {
        format!(" <span class=\"balloon\">{files_count}</span>")
    } else {
        String::new()
    };
    let main = format!(
        r##"<div class="row typecho-page-main typecho-post-area" role="form">
<form action="{action}" method="post" name="{form_name}">
<div class="col-mb-12 col-tb-9" role="main">
{draft}
<p class="title"><label for="title" class="sr-only">标题</label>
<input type="text" id="title" name="title" autocomplete="off" value="{title}" placeholder="标题" class="w-100 text title"></p>
<p class="mono url-slug"><label for="slug" class="sr-only">网址缩略名</label>{slug_html}</p>
<p><label for="text" class="sr-only">文章内容</label>
<textarea style="height:350px" autocomplete="off" id="text" name="text" class="w-100 mono">{text}</textarea></p>
{fields_html}
<p class="submit clearfix"><span class="left">
<button type="button" id="btn-cancel-preview" class="btn"><i class="i-caret-left"></i> 取消预览</button>
</span><span class="right">
{cid_field}
<input type="hidden" name="type" value="{kind}">
<button type="button" id="btn-preview" class="btn"><i class="i-exlink"></i> {preview_label}</button>
<button type="submit" name="do" value="save" id="btn-save" class="btn">保存草稿</button>
<button type="submit" name="do" value="publish" class="btn primary" id="btn-submit">{publish_label}</button>
{markdown_input}
</span></p>
</div>
<div id="edit-secondary" class="col-mb-12 col-tb-3" role="complementary">
<ul class="typecho-option-tabs clearfix">
<li class="active w-50"><a href="#tab-advance">选项</a></li>
<li class="w-50"><a href="#tab-files" id="tab-files-btn">附件{files_balloon}</a></li>
</ul>
<div id="tab-advance" class="tab-content">
<section class="typecho-post-option"><label for="date" class="typecho-label">发布日期</label>
<p><input class="typecho-date w-100" type="text" name="date" id="date" autocomplete="off" value="{date}"></p></section>
{cat_html}
<section class="typecho-post-option"><label for="tags" class="typecho-label">标签</label>
<p><input id="tags" name="tags" type="text" value="{tags}" class="w-100 text"></p></section>
<button type="button" id="advance-panel-btn" class="btn btn-xs">高级选项 <i class="i-caret-down"></i></button>
<div id="advance-panel">
{visibility_html}
<section class="typecho-post-option allow-option"><label class="typecho-label">权限控制</label><ul>
<li><input id="allowComment" name="allowComment" type="checkbox" value="1"{c1}><label for="allowComment">允许评论</label></li>
<li><input id="allowPing" name="allowPing" type="checkbox" value="1"{c2}><label for="allowPing">允许被引用</label></li>
<li><input id="allowFeed" name="allowFeed" type="checkbox" value="1"{c3}><label for="allowFeed">允许在聚合中出现</label></li>
</ul></section>
{trackback_html}
</div>
{edited_note}
</div>
<div id="tab-files" class="tab-content hidden">
{files_html}
</div>
</div>
</form>
<form class="hidden" id="side-upload" method="post" action="/action/upload" enctype="multipart/form-data">
<input type="hidden" name="parent" value="{parent}">
<input type="hidden" name="ajax" value="1">
<input type="file" name="file" id="upload-file" multiple>
</form>
</div>"##,
        title = html_escape(title),
        text = html_escape(display_text),
        date = html_escape(&date_text),
        tags = html_escape(&tags),
        c1 = check(allow_comment),
        c2 = check(allow_ping),
        c3 = check(allow_feed),
    );
    layout(&site, &user, key, &page_title, None, "container", &main)
}

async fn content_save_new(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    save_kind(&state, &headers, "post", None, &body).await
}
async fn page_save_new(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    save_kind(&state, &headers, "page", None, &body).await
}
async fn content_save(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>, body: String) -> Response {
    save_kind(&state, &headers, "post", Some(cid), &body).await
}
async fn page_save(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>, body: String) -> Response {
    save_kind(&state, &headers, "page", Some(cid), &body).await
}

async fn save_kind(state: &AppState, headers: &HeaderMap, kind: &str, cid: Option<i64>, body: &str) -> Response {
    let cid = cid.filter(|n| *n > 0);
    let need_rank = if kind == "page" { "editor" } else { "contributor" };
    let Ok((site, user, _)) = need(state, headers, need_rank).await else {
        return redirect_login();
    };
    let form = pairs(body);
    let many = repeated(body);
    let existing = if let Some(cid) = cid {
        match state.db.content_by_id(cid).await {
            Ok(Some(post)) if user.pass("editor") || post.author_id == user.uid => Some(post),
            Ok(Some(_)) => return plain(StatusCode::FORBIDDEN, "不能编辑这条内容"),
            _ => return plain(StatusCode::NOT_FOUND, "内容不存在"),
        }
    } else {
        None
    };
    let (mut status, mut password) = resolve_status(&form, &user, existing.as_ref().map(|p| p.status.as_str()).unwrap_or("draft"));
    if password.is_empty() {
        if let Some(old) = existing.as_ref() {
            if form.get("visibility").is_none() && form.get("password").is_none() {
                password = old.password.clone();
            }
        }
    }
    if !user.pass("editor") && matches!(status.as_str(), "publish" | "hidden" | "private") {
        status = "waiting".into();
    }
    let raw_date = form.get("date").map(|s| s.as_str()).filter(|s| !s.is_empty()).or_else(|| form.get("created").map(|s| s.as_str())).unwrap_or("");
    let created = parse_local(raw_date, site.timezone())
        .unwrap_or_else(|| existing.as_ref().map(|p| p.created).unwrap_or_else(|| chrono::Utc::now().timestamp()));
    let categories = if many.contains_key("category[]") {
        many.get("category[]").cloned().unwrap_or_default().iter().filter_map(|n| n.parse().ok()).collect()
    } else {
        form.get("categories").map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect()).unwrap_or_else(|| existing.as_ref().map(|p| p.categories.iter().map(|c| c.mid).collect()).unwrap_or_default())
    };
    let tags = if form.contains_key("tags") {
        form.get("tags").map(|s| s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()).unwrap_or_default()
    } else {
        existing.as_ref().map(|p| p.tags.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
    };
    let submitted = form.contains_key("do") || form.contains_key("title");
    let allow = |key: &str, old: bool| if submitted { form.contains_key(key) } else { old };
    let mut text = form.get("text").cloned().unwrap_or_else(|| existing.as_ref().map(|p| p.text.clone()).unwrap_or_default());
    if form.get("markdown").is_some_and(|v| v != "0") {
        let own = state.db.user_options(user.uid).await.unwrap_or_default();
        if option_flag(&own, "markdown", false) && !text.starts_with("<!--markdown-->") {
            text = format!("<!--markdown-->{text}");
        }
    }
    if form.get("do").map(|s| s.as_str()) == Some("publish") {
        let _targets = crate::write::trackback_targets(form.get("trackback").map(|s| s.as_str()).unwrap_or(""));
    }
    let saved = state
        .db
        .save_content(&ContentInput {
            cid: existing.as_ref().map(|p| p.cid),
            title: form.get("title").cloned().unwrap_or_else(|| existing.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| "未命名".into())),
            slug: form.get("slug").cloned().unwrap_or_else(|| existing.as_ref().map(|p| p.slug.clone()).unwrap_or_default()),
            text,
            created,
            kind: kind.into(),
            status,
            password,
            allow_comment: allow("allowComment", existing.as_ref().map(|p| p.allow_comment).unwrap_or(true)),
            allow_ping: allow("allowPing", existing.as_ref().map(|p| p.allow_ping).unwrap_or(true)),
            allow_feed: allow("allowFeed", existing.as_ref().map(|p| p.allow_feed).unwrap_or(true)),
            author_id: existing.as_ref().map(|p| p.author_id).unwrap_or(user.uid),
            parent: 0,
            sort_order: existing.as_ref().map(|p| p.sort_order).unwrap_or(0),
            categories,
            tags,
        })
        .await;
    match saved {
        Ok(id) => {
            if many.contains_key("fieldNames[]") || many.contains_key("fieldTypes[]") || many.contains_key("fieldValues[]") {
                let _ = state.db.replace_fields(id, &parse_fields(&many)).await;
            }
            if let Some(ids) = many.get("attachment[]") {
                let ids: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
                let _ = state.db.attach_files(id, &ids).await;
            }
            let dest = if kind == "page" {
                format!("/admin/write-page.php?cid={id}")
            } else {
                format!("/admin/write-post.php?cid={id}")
            };
            redirect(&dest)
        }
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "保存失败"),
    }
}

fn resolve_status(form: &HashMap<String, String>, _user: &SessionUser, fallback: &str) -> (String, String) {
    let password = form.get("password").cloned().unwrap_or_default();
    if let Some(vis) = form.get("visibility") {
        let mut status = match vis.as_str() {
            "hidden" => "hidden",
            "private" => "private",
            "waiting" => "waiting",
            "password" | "publish" => "publish",
            other => other,
        }
        .to_string();
        let mut password = if vis == "password" { password } else { String::new() };
        if form.get("do").map(|s| s.as_str()) == Some("save") {
            status = "draft".into();
            password.clear();
        }
        return (status, password);
    }
    if let Some(status) = form.get("status").cloned() {
        return (status, password);
    }
    match form.get("do").map(|s| s.as_str()) {
        Some("save") => ("draft".into(), String::new()),
        Some("publish") => ("publish".into(), password),
        _ => (fallback.into(), password),
    }
}

async fn posts_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    content_batch(&state, &headers, "post", "/admin/manage-posts.php", &q, &body).await
}
async fn pages_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    content_batch(&state, &headers, "page", "/admin/manage-pages.php", &q, &body).await
}
async fn media_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let action = q.get("do").map(|s| s.as_str()).unwrap_or("");
    if action == "clear" {
        let rows = state.db.list_contents("attachment", 500).await.unwrap_or_default();
        let mut n = 0i64;
        for row in rows.into_iter().filter(|p| p.parent == 0 && (user.pass("editor") || p.author_id == user.uid)) {
            remove_upload_file(&state, &row.text);
            if state.db.delete_content(row.cid).await.is_ok() {
                n += 1;
            }
        }
        let notice = if n > 0 { "未归档文件已经被清理" } else { "没有未归档文件被清理" };
        return notice_to("/admin/manage-medias.php", notice);
    }
    let ids: Vec<i64> = repeated(&body)
        .get("cid[]")
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    if action == "delete" {
        let mut n = 0i64;
        for cid in ids {
            let Ok(Some(post)) = state.db.content_by_id(cid).await else { continue };
            if post.kind != "attachment" {
                continue;
            }
            if !user.pass("editor") && post.author_id != user.uid {
                continue;
            }
            remove_upload_file(&state, &post.text);
            if state.db.delete_content(cid).await.is_ok() {
                n += 1;
            }
        }
        let notice = if n > 0 { "文件已经被删除" } else { "没有文件被删除" };
        return notice_to("/admin/manage-medias.php", notice);
    }
    redirect("/admin/manage-medias.php")
}

async fn content_batch(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    back: &str,
    q: &HashMap<String, String>,
    body: &str,
) -> Response {
    let need_rank = if kind == "page" { "editor" } else { "contributor" };
    let Ok((_, user, _)) = need(state, headers, need_rank).await else {
        return redirect_login();
    };
    let ids: Vec<i64> = repeated(body).get("cid[]").cloned().unwrap_or_default().iter().filter_map(|s| s.parse().ok()).collect();
    let action = q.get("do").map(|s| s.as_str()).unwrap_or("");
    let mark = q.get("status").map(|s| s.as_str()).unwrap_or("");
    for cid in ids {
        let Ok(Some(post)) = state.db.content_by_id(cid).await else { continue };
        if post.kind != kind && !(kind == "post" && post.kind == "post") {
            continue;
        }
        if !user.pass("editor") && post.author_id != user.uid {
            continue;
        }
        if action == "delete" {
            let _ = state.db.delete_content(cid).await;
        } else if action == "mark" && user.pass("editor") {
            let _ = state.db.set_content_status(cid, mark).await;
        }
    }
    redirect(back)
}

async fn content_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>) -> Response {
    delete_kind(&state, &headers, cid, "/admin/manage-posts.php").await
}
async fn page_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>) -> Response {
    delete_kind(&state, &headers, cid, "/admin/manage-pages.php").await
}
async fn media_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>, body: String) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let ajax = pairs(&body).contains_key("ajax");
    let Ok(Some(post)) = state.db.content_by_id(cid).await else {
        if ajax {
            return json_raw("{\"code\":500,\"message\":\"没有文件被删除\"}");
        }
        return notice_to("/admin/manage-medias.php", "没有文件被删除");
    };
    if post.kind != "attachment" || (!user.pass("editor") && post.author_id != user.uid) {
        if ajax {
            return json_raw("{\"code\":500,\"message\":\"没有文件被删除\"}");
        }
        return plain(StatusCode::FORBIDDEN, "不能删除这个文件");
    }
    remove_upload_file(&state, &post.text);
    let ok = state.db.delete_content(cid).await.is_ok();
    if ajax {
        let body = if ok {
            "{\"code\":200,\"message\":\"文件已经被删除\"}"
        } else {
            "{\"code\":500,\"message\":\"没有文件被删除\"}"
        };
        return json_raw(body);
    }
    notice_to("/admin/manage-medias.php", if ok { "文件已经被删除" } else { "没有文件被删除" })
}

async fn delete_kind(state: &AppState, headers: &HeaderMap, cid: i64, back: &str) -> Response {
    let Ok((_, user, _)) = need(state, headers, "contributor").await else {
        return redirect_login();
    };
    let Ok(Some(post)) = state.db.content_by_id(cid).await else {
        return plain(StatusCode::NOT_FOUND, "内容不存在");
    };
    if post.kind == "page" && !user.pass("editor") {
        return plain(StatusCode::FORBIDDEN, "独立页需要编辑权限");
    }
    if !user.pass("editor") && post.author_id != user.uid {
        return plain(StatusCode::FORBIDDEN, "不能删除这条内容");
    }
    let _ = state.db.delete_content(cid).await;
    redirect(back)
}

async fn categories(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    category_list(&state, &headers, &q).await
}
async fn tags(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    meta_page(&state, &headers, "tag", "管理标签", "tags", "/admin/manage-tags.php", "/admin/manage-tags.php", &q).await
}

async fn meta_page(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    title: &str,
    key: &str,
    base: &str,
    back: &str,
    q: &HashMap<String, String>,
) -> Response {
    let Ok((site, user, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let keywords = q.get("keywords").map(|s| s.as_str()).unwrap_or("");
    let rows = state.db.list_metas(kind).await.unwrap_or_default();
    let rows: Vec<_> = rows.into_iter().filter(|m| keywords.is_empty() || title_hit(&m.name, keywords)).collect();
    let mut body_rows = String::new();
    if rows.is_empty() {
        body_rows.push_str("<tr><td colspan=\"5\"><h6 class=\"typecho-list-table-title\">没有任何项目</h6></td></tr>");
    }
    for row in &rows {
        let link = if kind == "category" {
            links::category_path(&site, row)
        } else {
            links::tag_path(&site, row)
        };
        body_rows.push_str(&format!(
            r#"<tr id="mid-{mid}"><td class="kit-hidden-mb"><input type="checkbox" value="{mid}" name="mid[]"></td>
<td><a href="{base}?mid={mid}">{name}</a> <a href="{link}"><i class="i-exlink"></i></a></td>
<td class="kit-hidden-mb">{slug}</td><td>{count}</td>
<td><form method="post" action="/action/metas-tag-edit?do=delete&mid={mid}"><button class="btn btn-s" type="submit">删除</button></form></td></tr>"#,
            mid = row.mid,
            name = html_escape(&row.name),
            link = html_escape(&link),
            slug = html_escape(&row.slug),
            count = row.count,
        ));
    }
    let parent = if kind == "category" {
        "<li><label class=\"typecho-label\">父级</label><input class=\"text-s\" name=\"parent\" value=\"0\"><p class=\"description\">父分类的编号，0 表示无父级。</p></li>"
    } else {
        ""
    };
    let main = format!(
        r#"<div class="row typecho-page-main manage-metas"><div class="col-mb-12" role="main">
<div class="typecho-list-operate clearfix"><form method="get"><div class="operate">
<label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button">选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu"><li><a lang="你确认要删除这些项目吗?" href="/action/metas-tag-edit?do=delete">删除</a></li></ul></div>
</div><div class="search"><input class="text-s" name="keywords" placeholder="请输入关键字" value="{kw}"><button class="btn btn-s" type="submit">筛选</button></div></form></div>
<form method="post" class="operate-form" action="/action/metas-tag-edit"><div class="typecho-table-wrap"><table class="typecho-list-table">
<thead><tr><th class="kit-hidden-mb"></th><th>名称</th><th class="kit-hidden-mb">缩略名</th><th>数量</th><th></th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form>
<form method="post" action="/action/metas-tag-edit"><ul class="typecho-option" id="custom-field">
<li><label class="typecho-label">名称</label><input class="text w-100" name="name" required></li>
<li><label class="typecho-label">缩略名</label><input class="text w-100 mono" name="slug"></li>
<li><label class="typecho-label">描述</label><textarea name="description"></textarea></li>
{parent}
<li class="typecho-option-submit"><button type="submit" class="btn primary">保存</button></li>
</ul></form>
</div></div>"#,
        kw = html_escape(keywords)
    );
    layout(&site, &user, key, title, None, "container", &main)
}

async fn category_save(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    category_commit(&state, &headers, None, &body).await
}
async fn category_save_new(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    category_commit(&state, &headers, None, &body).await
}
async fn category_save_edit(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(mid): Path<i64>, body: String) -> Response {
    category_commit(&state, &headers, Some(mid), &body).await
}
async fn tag_save(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    meta_save(&state, &headers, "tag", "/admin/manage-tags.php", &body).await
}

async fn meta_save(state: &AppState, headers: &HeaderMap, kind: &str, back: &str, body: &str) -> Response {
    let Ok((_, _, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let form = pairs(body);
    let _ = state
        .db
        .save_meta(&MetaInput {
            mid: form.get("mid").and_then(|s| s.parse().ok()),
            name: form.get("name").cloned().unwrap_or_default(),
            slug: form.get("slug").cloned().unwrap_or_default(),
            kind: kind.into(),
            description: form.get("description").cloned().unwrap_or_default(),
            parent: form.get("parent").and_then(|s| s.parse().ok()).unwrap_or(0),
        })
        .await;
    redirect(back)
}

async fn categories_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "editor").await else {
        return redirect_login();
    };
    let mids: Vec<i64> = repeated(&body)
        .get("mid[]")
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    let back = "/admin/manage-categories.php";
    match q.get("do").map(|s| s.as_str()).unwrap_or("") {
        "delete" => {
            let n = state.db.delete_categories(&mids).await.unwrap_or(0);
            let notice = if n > 0 { "分类已经删除" } else { "没有分类被删除" };
            notice_to(back, notice)
        }
        "refresh" => {
            if mids.is_empty() {
                notice_to(back, "没有选择任何分类")
            } else {
                let _ = state.db.refresh_categories(&mids).await;
                notice_to(back, "分类刷新已经完成")
            }
        }
        "merge" => {
            let form = pairs(&body);
            let merge = form.get("merge").map(|s| s.as_str()).unwrap_or("");
            if merge.is_empty() {
                return error_to(back, "分类主键不存在");
            }
            let Ok(target) = merge.parse::<i64>() else {
                return error_to(back, "请选择需要合并的分类");
            };
            let exists = state
                .db
                .meta_by_id(target)
                .await
                .ok()
                .flatten()
                .is_some_and(|m| m.kind == "category");
            if !exists {
                return error_to(back, "请选择需要合并的分类");
            }
            if mids.is_empty() {
                notice_to(back, "没有选择任何分类")
            } else {
                let _ = state.db.merge_categories(target, &mids).await;
                notice_to(back, "分类已经合并")
            }
        }
        _ => redirect(back),
    }
}
async fn tags_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    meta_batch(&state, &headers, "/admin/manage-tags.php", &q, &body).await
}

async fn meta_batch(state: &AppState, headers: &HeaderMap, back: &str, q: &HashMap<String, String>, body: &str) -> Response {
    let Ok((_, _, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    if q.get("do").map(|s| s.as_str()) == Some("delete") {
        for mid in repeated(body).get("mid[]").cloned().unwrap_or_default() {
            if let Ok(mid) = mid.parse::<i64>() {
                let _ = state.db.delete_meta(mid).await;
            }
        }
    }
    redirect(back)
}

async fn category_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(mid): Path<i64>) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "editor").await else {
        return redirect_login();
    };
    let n = state.db.delete_categories(&[mid]).await.unwrap_or(0);
    let notice = if n > 0 { "分类已经删除" } else { "没有分类被删除" };
    notice_to("/admin/manage-categories.php", notice)
}

async fn category_new(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    category_form(&state, &headers, None, &q, &HashMap::new()).await
}

async fn category_edit(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(mid): Path<i64>, Query(q): Query<HashMap<String, String>>) -> Response {
    category_form(&state, &headers, Some(mid), &q, &HashMap::new()).await
}

async fn category_default(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(mid): Path<i64>) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "editor").await else {
        return redirect_login();
    };
    let Some(meta) = state.db.meta_by_id(mid).await.ok().flatten().filter(|m| m.kind == "category") else {
        return error_to("/admin/manage-categories.php", "分类不存在");
    };
    let _ = state.db.set_option("defaultCategory", &mid.to_string()).await;
    notice_to("/admin/manage-categories.php", &format!("{} 已经被设为默认分类", meta.name))
}
async fn tag_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(mid): Path<i64>) -> Response {
    meta_delete(&state, &headers, mid, "/admin/manage-tags.php").await
}

async fn meta_delete(state: &AppState, headers: &HeaderMap, mid: i64, back: &str) -> Response {
    let Ok((_, _, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let _ = state.db.delete_meta(mid).await;
    redirect(back)
}

fn notice_to(path: &str, notice: &str) -> Response {
    let join = if path.contains('?') { '&' } else { '?' };
    redirect(&format!("{path}{join}notice={}", urlencoding::encode(notice)))
}

fn error_to(path: &str, error: &str) -> Response {
    let join = if path.contains('?') { '&' } else { '?' };
    redirect(&format!("{path}{join}error={}", urlencoding::encode(error)))
}

async fn category_list(state: &AppState, headers: &HeaderMap, q: &HashMap<String, String>) -> Response {
    let Ok((site, user, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let all = state.db.list_metas("category").await.unwrap_or_default();
    let parent_id = q.get("parent").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    let parent_meta = if parent_id > 0 {
        match all.iter().find(|m| m.mid == parent_id) {
            Some(meta) => Some(meta.clone()),
            None => return plain(StatusCode::NOT_FOUND, "分类不存在"),
        }
    } else {
        None
    };
    let mut rows: Vec<_> = all.iter().filter(|m| m.parent == parent_id).cloned().collect();
    rows.sort_by_key(|m| (m.sort_order, m.mid));
    let default_mid = site.get("defaultCategory").parse::<i64>().unwrap_or(0);
    let mut options = String::new();
    for row in &rows {
        options.push_str(&format!(
            "<option value=\"{mid}\">{name}</option>",
            mid = row.mid,
            name = html_escape(&row.name)
        ));
    }
    let mut body_rows = String::new();
    if rows.is_empty() {
        body_rows.push_str("<tr><td colspan=\"6\"><h6 class=\"typecho-list-table-title\">没有任何分类</h6></td></tr>");
    }
    for row in &rows {
        let children = all.iter().filter(|m| m.parent == row.mid).count();
        let child_cell = if children > 0 {
            let label = if children == 1 {
                "一个分类".to_string()
            } else {
                format!("{children}个分类")
            };
            format!(
                "<a href=\"/admin/manage-categories.php?parent={mid}\">{label}</a>",
                mid = row.mid
            )
        } else {
            format!("<a href=\"/admin/category.php?parent={mid}\">新增</a>", mid = row.mid)
        };
        let default_cell = if default_mid == row.mid {
            "默认".to_string()
        } else {
            format!(
                "<a class=\"hidden-by-mouse\" href=\"/action/metas-category-edit?do=default&mid={mid}\" title=\"设为默认\">默认</a>",
                mid = row.mid
            )
        };
        let link = links::category_path(&site, row);
        let size = count_size(row.count);
        body_rows.push_str(&format!(
            r#"<tr id="mid-{mid}"><td class="kit-hidden-mb"><input type="checkbox" value="{mid}" name="mid[]"></td>
<td><a href="/admin/category.php?mid={mid}">{name}</a> <a href="{link}" title="浏览 {name}"><i class="i-exlink"></i></a></td>
<td>{child_cell}</td>
<td class="kit-hidden-mb">{slug}</td>
<td>{default_cell}</td>
<td class="kit-hidden-mb"><a class="balloon-button left size-{size}" href="/admin/manage-posts.php?category={mid}">{count}</a></td></tr>"#,
            mid = row.mid,
            name = html_escape(&row.name),
            link = html_escape(&link),
            slug = html_escape(&row.slug),
            count = row.count,
        ));
    }
    let back = if let Some(meta) = &parent_meta {
        let href = if meta.parent > 0 {
            format!("/admin/manage-categories.php?parent={}", meta.parent)
        } else {
            "/admin/manage-categories.php".into()
        };
        format!("<a href=\"{href}\">&laquo; 返回父级分类</a>")
    } else {
        String::new()
    };
    let add = if parent_id > 0 {
        format!("/admin/category.php?parent={parent_id}")
    } else {
        "/admin/category.php".into()
    };
    let title = if let Some(meta) = &parent_meta {
        format!("管理 {} 的子分类", meta.name)
    } else {
        "管理分类".into()
    };
    let main = format!(
        r#"<div class="row typecho-page-main manage-metas"><div class="col-mb-12" role="main">
{flash}
<form method="post" name="manage_categories" class="operate-form">
<div class="typecho-list-operate clearfix"><div class="operate">
<label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button"><i class="sr-only">操作</i>选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu">
<li><a lang="此分类下的所有内容将被删除, 你确认要删除这些分类吗?" href="/action/metas-category-edit?do=delete">删除</a></li>
<li><a lang="刷新分类可能需要等待较长时间, 你确认要刷新这些分类吗?" href="/action/metas-category-edit?do=refresh">刷新</a></li>
<li class="multiline"><button type="button" class="btn merge btn-s" rel="/action/metas-category-edit?do=merge">合并到</button>
<select name="merge">{options}</select></li>
</ul></div></div>
<div class="search" role="search">{back}</div></div>
<div class="typecho-table-wrap"><table class="typecho-list-table">
<colgroup><col width="20" class="kit-hidden-mb"><col width=""><col width="15%" class="kit-hidden-mb"><col width="25%"><col width="15%"><col width="10%" class="kit-hidden-mb"></colgroup>
<thead><tr class="nodrag"><th class="kit-hidden-mb"></th><th>名称</th><th>子分类</th><th class="kit-hidden-mb">缩略名</th><th></th><th class="kit-hidden-mb">文章数</th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form></div></div>"#,
        flash = flash_html(q),
    );
    layout(&site, &user, "categories", &title, Some(&add), "container", &main)
}

async fn category_form(
    state: &AppState,
    headers: &HeaderMap,
    mid: Option<i64>,
    q: &HashMap<String, String>,
    posted: &HashMap<String, String>,
) -> Response {
    let Ok((site, user, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let all = state.db.list_metas("category").await.unwrap_or_default();
    let current = if let Some(mid) = mid {
        match all.iter().find(|m| m.mid == mid) {
            Some(meta) => Some(meta.clone()),
            None if posted.is_empty() => return redirect("/admin/manage-categories.php"),
            None => None,
        }
    } else {
        None
    };
    let parent_query = q.get("parent").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    if current.is_none() && parent_query > 0 && !all.iter().any(|m| m.mid == parent_query) {
        return plain(StatusCode::NOT_FOUND, "分类不存在");
    }
    let name = posted
        .get("name")
        .cloned()
        .or_else(|| current.as_ref().map(|m| m.name.clone()))
        .unwrap_or_default();
    let slug = posted
        .get("slug")
        .cloned()
        .or_else(|| current.as_ref().map(|m| m.slug.clone()))
        .unwrap_or_default();
    let description = posted
        .get("description")
        .cloned()
        .or_else(|| current.as_ref().map(|m| m.description.clone()))
        .unwrap_or_default();
    let parent = posted
        .get("parent")
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| current.as_ref().map(|m| m.parent))
        .unwrap_or(parent_query);
    let mut select = String::from("<option value=\"0\">不选择</option>");
    for row in category_tree(&all) {
        if let Some(mid) = mid {
            if category_is_under(&all, row.mid, mid) {
                continue;
            }
        }
        let level = category_depth(&all, row.mid);
        let indent = "&nbsp;".repeat(level * 4);
        let sel = if row.mid == parent { " selected" } else { "" };
        select.push_str(&format!(
            "<option value=\"{mid}\"{sel}>{indent}{name}</option>",
            mid = row.mid,
            name = html_escape(&row.name)
        ));
    }
    let action = "/action/metas-category-edit";
    let (do_value, submit, title) = if let Some(meta) = &current {
        ("update", "编辑分类", format!("编辑分类 {}", meta.name))
    } else if parent_query > 0 {
        let parent_name = all.iter().find(|m| m.mid == parent_query).map(|m| m.name.as_str()).unwrap_or("");
        ("insert", "增加分类", format!("新增 {parent_name} 的子分类"))
    } else {
        ("insert", "增加分类", "新增分类".into())
    };
    let mid_value = current.as_ref().map(|m| m.mid.to_string()).unwrap_or_default();
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-6 col-tb-offset-3">
{flash}
<form method="post" action="{action}" enctype="application/x-www-form-urlencoded">
<ul class="typecho-option"><li><label class="typecho-label" for="name">分类名称 *</label>
<input id="name" name="name" type="text" class="text" value="{name}"></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="slug">分类缩略名</label>
<input id="slug" name="slug" type="text" class="text" value="{slug}">
<p class="description">分类缩略名用于创建友好的链接形式, 建议使用字母, 数字, 下划线和横杠.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="parent">父级分类</label>
<select id="parent" name="parent">{select}</select>
<p class="description">此分类将归档在您选择的父级分类下.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="description">分类描述</label>
<textarea id="description" name="description">{description}</textarea>
<p class="description">此文字用于描述分类, 在有的主题中它会被显示.</p></li></ul>
<input type="hidden" name="do" value="{do_value}">
<input type="hidden" name="mid" value="{mid_value}">
<ul class="typecho-option typecho-option-submit"><li><button type="submit" class="btn primary">{submit}</button></li></ul>
</form></div></div>"#,
        flash = flash_html(q),
        name = html_escape(&name),
        slug = html_escape(&slug),
        description = html_escape(&description),
    );
    layout(&site, &user, "categories", &title, None, "container", &main)
}

async fn category_commit(state: &AppState, headers: &HeaderMap, path_mid: Option<i64>, body: &str) -> Response {
    let Ok((_, _, _)) = need(state, headers, "editor").await else {
        return redirect_login();
    };
    let form = pairs(body);
    let updating = form.get("do").map(|s| s.as_str()) == Some("update");
    let mid = if updating {
        path_mid.or_else(|| form.get("mid").and_then(|s| s.parse::<i64>().ok()).filter(|n| *n > 0))
    } else {
        None
    };
    let mut errors = Vec::new();
    if updating {
        match mid {
            None => errors.push("分类主键不存在".to_string()),
            Some(mid) => {
                let exists = state
                    .db
                    .meta_by_id(mid)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|m| m.kind == "category");
                if !exists {
                    errors.push("分类不存在".to_string());
                }
            }
        }
    }
    let name = form.get("name").cloned().unwrap_or_default();
    let slug_input = form.get("slug").cloned().unwrap_or_default();
    if name.trim().is_empty() {
        errors.push("必须填写分类名称".into());
    } else if !auth::xss_ok(&name) {
        errors.push("请不要在分类名称中使用特殊字符".into());
    }
    if slug_input.trim().is_empty() {
        let generated = crate::write::typecho_slug(&name);
        if generated.is_empty() || state.db.category_slug_taken(&generated, mid.unwrap_or(0)).await.unwrap_or(false) {
            errors.push("分类名称无法被转换为缩略名".into());
        }
    } else if !auth::xss_ok(&slug_input) {
        errors.push("请不要在缩略名中使用特殊字符".into());
    }
    let slug = crate::write::typecho_slug(if slug_input.trim().is_empty() { name.trim() } else { slug_input.trim() });
    let except = mid.unwrap_or(0);
    if !name.trim().is_empty() && state.db.category_name_taken(name.trim(), except).await.unwrap_or(false) {
        errors.push("分类名称已经存在".into());
    }
    if !slug.is_empty() && state.db.category_slug_taken(&slug, except).await.unwrap_or(false) {
        if !(slug_input.trim().is_empty() && errors.iter().any(|e| e.contains("无法被转换为缩略名"))) {
            errors.push("缩略名已经存在".into());
        }
    }
    let mut parent = form.get("parent").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0).max(0);
    let all = state.db.list_metas("category").await.unwrap_or_default();
    if let Some(mid) = mid {
        if category_is_under(&all, parent, mid) {
            parent = all.iter().find(|m| m.mid == mid).map(|m| m.parent).unwrap_or(0);
        }
    }
    if parent > 0 && !all.iter().any(|m| m.mid == parent) {
        parent = 0;
    }
    if !errors.is_empty() {
        let mut q = HashMap::new();
        q.insert("error".into(), errors.join("\n"));
        if !updating {
            if let Some(p) = form.get("parent").cloned() {
                q.insert("parent".into(), p);
            }
        }
        return category_form(state, headers, mid, &q, &form).await;
    }
    let saved = state
        .db
        .save_category(&crate::write::MetaInput {
            mid,
            name: name.trim().to_string(),
            slug: slug_input,
            kind: "category".into(),
            description: form.get("description").cloned().unwrap_or_default(),
            parent,
        })
        .await;
    match saved {
        Ok(_) => {
            let verb = if updating { "已经被更新" } else { "已经被增加" };
            let notice = format!("分类 {} {verb}", name.trim());
            let back = if parent > 0 {
                format!("/admin/manage-categories.php?parent={parent}")
            } else {
                "/admin/manage-categories.php".into()
            };
            notice_to(&back, &notice)
        }
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "保存分类失败"),
    }
}

fn category_tree(rows: &[crate::model::Meta]) -> Vec<&crate::model::Meta> {
    fn walk<'a>(rows: &'a [crate::model::Meta], parent: i64, out: &mut Vec<&'a crate::model::Meta>) {
        let mut kids: Vec<_> = rows.iter().filter(|m| m.parent == parent).collect();
        kids.sort_by_key(|m| (m.sort_order, m.mid));
        for kid in kids {
            out.push(kid);
            walk(rows, kid.mid, out);
        }
    }
    let mut out = Vec::new();
    walk(rows, 0, &mut out);
    for row in rows {
        if !out.iter().any(|m| m.mid == row.mid) {
            out.push(row);
        }
    }
    out
}

fn category_depth(rows: &[crate::model::Meta], mid: i64) -> usize {
    let mut depth = 0;
    let mut parent = rows.iter().find(|m| m.mid == mid).map(|m| m.parent).unwrap_or(0);
    let mut guard = 0;
    while parent != 0 && guard < 32 {
        depth += 1;
        parent = rows.iter().find(|m| m.mid == parent).map(|m| m.parent).unwrap_or(0);
        guard += 1;
    }
    depth
}

fn category_is_under(rows: &[crate::model::Meta], node: i64, ancestor: i64) -> bool {
    if node == 0 {
        return false;
    }
    if node == ancestor {
        return true;
    }
    let mut parent = rows.iter().find(|m| m.mid == node).map(|m| m.parent).unwrap_or(0);
    let mut guard = 0;
    while parent != 0 && guard < 32 {
        if parent == ancestor {
            return true;
        }
        parent = rows.iter().find(|m| m.mid == parent).map(|m| m.parent).unwrap_or(0);
        guard += 1;
    }
    false
}

fn split_by_count(count: i64, sizes: &[i64]) -> i64 {
    for size in sizes {
        if count < *size {
            return *size;
        }
    }
    0
}

fn count_size(n: i64) -> usize {
    if n < 1 {
        0
    } else if n < 10 {
        1
    } else if n < 20 {
        2
    } else if n < 50 {
        3
    } else if n < 100 {
        4
    } else {
        5
    }
}

async fn comments(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let rows = state.db.list_comments(500).await.unwrap_or_default();
    let posts = state.db.list_contents("post", 500).await.unwrap_or_default();
    let pages = state.db.list_contents("page", 200).await.unwrap_or_default();
    let mut titles: HashMap<i64, String> = HashMap::new();
    let mut owners: HashMap<i64, i64> = HashMap::new();
    for p in posts.iter().chain(pages.iter()) {
        titles.insert(p.cid, p.title.clone());
        owners.insert(p.cid, p.author_id);
    }
    let status = q.get("status").map(|s| s.as_str()).unwrap_or("approved");
    let keywords = q.get("keywords").map(|s| s.as_str()).unwrap_or("");
    let cid = q.get("cid").and_then(|s| s.parse::<i64>().ok());
    let all = q.get("__typecho_all_comments").map(|s| s == "on").unwrap_or(user.pass("editor"));
    let filtered: Vec<_> = rows
        .iter()
        .filter(|c| c.status == status || (status == "approved" && q.get("status").is_none() && c.status == "approved"))
        .filter(|c| cid.is_none_or(|id| c.cid == id))
        .filter(|c| {
            if user.pass("editor") && all {
                true
            } else {
                owners.get(&c.cid).copied().unwrap_or(c.owner_id) == user.uid
            }
        })
        .filter(|c| keywords.is_empty() || title_hit(&c.text, keywords) || title_hit(&c.author, keywords))
        .collect();
    let page = q.get("page").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1);
    let pages_n = filtered.len().div_ceil(PAGE).max(1);
    let page = page.min(pages_n);
    let slice = &filtered[(page - 1) * PAGE..filtered.len().min(page * PAGE)];
    let waiting = rows.iter().filter(|c| c.status == "waiting").count();
    let spam = rows.iter().filter(|c| c.status == "spam").count();
    let mut tabs = String::new();
    for (name, label, n) in [("approved", "已通过", 0usize), ("waiting", "待审核", waiting), ("spam", "垃圾", spam)] {
        let current = if status == name { " class=\"current\"" } else { "" };
        let balloon = if n > 0 { format!(" <span class=\"balloon\">{n}</span>") } else { String::new() };
        tabs.push_str(&format!("<li{current}><a href=\"/admin/manage-comments.php?status={name}\">{label}{balloon}</a></li>"));
    }
    let mut body_rows = String::new();
    if slice.is_empty() {
        body_rows.push_str("<tr><td colspan=\"4\"><h6 class=\"typecho-list-table-title\">没有评论</h6></td></tr>");
    }
    for row in slice {
        let title = titles.get(&row.cid).map(|s| s.as_str()).unwrap_or("文章");
        let weak = |name: &str, label: &str| {
            if row.status == name {
                format!("<span class=\"weak\">{label}</span>")
            } else {
                format!("<a href=\"/action/comments-edit?do={name}&coid={}\">{label}</a>", row.coid)
            }
        };
        let payload = serde_json::json!({
            "author": row.author,
            "mail": row.mail,
            "url": row.url,
            "text": row.text,
            "type": row.kind,
            "ip": row.ip,
        });
        let data_comment = html_escape(&payload.to_string());
        let reply = if row.status == "approved" && row.kind == "comment" {
            format!(
                r##"<a href="#comment-{coid}" rel="/action/comments-edit?do=reply&coid={coid}" class="operate-reply">回复</a>"##,
                coid = row.coid
            )
        } else {
            String::new()
        };
        body_rows.push_str(&format!(
            r##"<tr id="comment-{coid}" data-comment="{data}"><td valign="top" class="kit-hidden-mb"><input type="checkbox" value="{coid}" name="coid[]"></td>
<td valign="top" class="kit-hidden-mb"><div class="comment-avatar">{avatar}</div></td>
<td valign="top" class="comment-head"><div class="comment-meta"><strong class="comment-author">{author}</strong><br><span>{mail}</span></div></td>
<td valign="top" class="comment-body"><div class="comment-date">{date} 于 <a href="/admin/manage-comments.php?cid={cid}">{title}</a></div>
<div class="comment-content"><p>{text}</p></div>
<div class="comment-action hidden-by-mouse">{pass} {wait} {spam} <a href="#comment-{coid}" rel="/action/comments-edit?do=edit&coid={coid}" class="operate-edit">编辑</a> {reply} <a class="operate-delete" lang="你确认要删除{author}的评论吗?" href="/action/comments-edit?do=delete&coid={coid}">删除</a></div>
</td></tr>"##,
            coid = row.coid,
            data = data_comment,
            avatar = gravatar(&state.cfg.gravatar_prefix, &row.mail),
            author = html_escape(&row.author),
            mail = html_escape(&row.mail),
            date = html_escape(&date_word(row.created, site.timezone())),
            cid = row.cid,
            title = html_escape(title),
            text = html_escape(&crate::text::truncate_chars(&row.text, 280)),
            pass = weak("approved", "通过"),
            wait = weak("waiting", "待审核"),
            spam = weak("spam", "垃圾"),
        ));
    }
    let pager = pager_html(page, pages_n, "/admin/manage-comments.php", &q);
    let main = format!(
        r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12 typecho-list">
<div class="clearfix"><ul class="typecho-option-tabs">{tabs}</ul></div>
<div class="typecho-list-operate clearfix"><form method="get"><div class="operate">
<label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button">选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu">
<li><a href="/action/comments-edit?do=approved">通过</a></li>
<li><a href="/action/comments-edit?do=waiting">待审核</a></li>
<li><a href="/action/comments-edit?do=spam">标记垃圾</a></li>
<li><a lang="你确认要删除这些评论吗?" href="/action/comments-edit?do=delete">删除</a></li>
</ul></div></div>
<div class="search" role="search"><input type="text" class="text-s" placeholder="请输入关键字" name="keywords" value="{kw}">
<input type="hidden" name="status" value="{status}"><button type="submit" class="btn btn-s">筛选</button></div>
</form></div>
<form method="post" class="operate-form" action="/action/comments-edit"><div class="typecho-table-wrap"><table class="typecho-list-table">
<colgroup><col width="3%" class="kit-hidden-mb"><col width="6%" class="kit-hidden-mb"><col width="20%"><col width="71%"></colgroup>
<thead><tr><th class="kit-hidden-mb"></th><th>作者</th><th class="kit-hidden-mb"></th><th>内容</th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form>
<div class="typecho-list-operate clearfix">{pager}</div>
</div></div>"#,
        kw = html_escape(keywords)
    );
    layout(&site, &user, "comments", "管理评论", None, "container", &main)
}

async fn comments_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let action = q.get("do").map(|s| s.as_str()).unwrap_or("");
    if action == "delete-spam" {
        if let Ok(rows) = state.db.list_comments(1000).await {
            for row in rows.into_iter().filter(|c| c.status == "spam") {
                let _ = state.db.delete_comment(row.coid).await;
            }
        }
    } else {
        for coid in repeated(&body).get("coid[]").cloned().unwrap_or_default() {
            if let Ok(coid) = coid.parse::<i64>() {
                apply_comment(&state, coid, action).await;
            }
        }
    }
    redirect("/admin/manage-comments.php")
}

async fn comment_get_act(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    if let Some(coid) = q.get("coid").and_then(|s| s.parse().ok()) {
        apply_comment(&state, coid, q.get("do").map(|s| s.as_str()).unwrap_or("")).await;
    }
    let status = q.get("status").map(|s| s.as_str()).unwrap_or("");
    if status.is_empty() {
        redirect("/admin/manage-comments.php")
    } else {
        redirect(&format!("/admin/manage-comments.php?status={status}"))
    }
}

async fn comment_act(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(coid): Path<i64>, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    let act = form.get("act").map(|s| s.as_str()).or_else(|| form.get("do").map(|s| s.as_str())).unwrap_or("");
    apply_comment(&state, coid, act).await;
    redirect("/admin/manage-comments.php")
}

async fn comment_edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let coid = q.get("coid").and_then(|s| s.parse().ok()).unwrap_or(0);
    let Some(row) = state.db.comment_by_coid(coid).await.unwrap_or(None) else {
        return comment_json(0, "", "修评论失败");
    };
    if !comment_writable(&user, row.owner_id) {
        return comment_json(0, "", "修评论失败");
    }
    let form = pairs(&body);
    let author = strip_tags(form.get("author").map(|s| s.as_str()).unwrap_or("")).trim().to_string();
    let mail = strip_tags(form.get("mail").map(|s| s.as_str()).unwrap_or("")).trim().to_string();
    let url = safe_comment_url(form.get("url").map(|s| s.as_str()).unwrap_or(""));
    let text = form.get("text").cloned().unwrap_or_default();
    if state
        .db
        .update_comment(coid, &author, &mail, &url, &text)
        .await
        .is_err()
    {
        return comment_json(0, "", "修评论失败");
    }
    let html = text::render_comment_html(&text, site.flag("commentsMarkdown", false));
    comment_json(1, &html, "")
}

async fn comment_reply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let coid = q.get("coid").and_then(|s| s.parse().ok()).unwrap_or(0);
    let Some(row) = state.db.comment_by_coid(coid).await.unwrap_or(None) else {
        return comment_json(0, "", "回复评论失败");
    };
    if !comment_writable(&user, row.owner_id) || row.status != "approved" || row.kind != "comment" {
        return comment_json(0, "", "回复评论失败");
    }
    let form = pairs(&body);
    let text = form.get("text").cloned().unwrap_or_default();
    let inserted = state
        .db
        .insert_comment(&crate::repo::NewComment {
            cid: row.cid,
            created: chrono::Utc::now().timestamp(),
            author: user.label(),
            author_id: user.uid,
            owner_id: row.owner_id,
            mail: user.mail.clone(),
            url: user.url.clone(),
            ip: String::new(),
            agent: String::new(),
            text: text.clone(),
            status: "approved".into(),
            kind: "comment".into(),
            parent: coid,
        })
        .await;
    match inserted {
        Ok(_) => {
            let html = text::render_comment_html(&text, site.flag("commentsMarkdown", false));
            comment_json(1, &html, "")
        }
        Err(_) => comment_json(0, "", "回复评论失败"),
    }
}

fn comment_writable(user: &SessionUser, owner_id: i64) -> bool {
    user.pass("editor") || owner_id == user.uid
}

fn comment_json(success: i32, content: &str, message: &str) -> Response {
    let body = if success == 1 {
        serde_json::json!({"success": 1, "comment": {"content": content}}).to_string()
    } else {
        serde_json::json!({"success": 0, "message": message}).to_string()
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "json"))
}

fn strip_tags(raw: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace('\0', "")
}

fn safe_comment_url(raw: &str) -> String {
    let raw = strip_tags(raw).trim().to_string();
    if raw.is_empty() {
        return String::new();
    }
    if auth::url_ok(&raw) && (raw.starts_with("http://") || raw.starts_with("https://")) {
        raw
    } else {
        String::new()
    }
}

async fn write_preview(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let form = pairs(&body);
    let kind = if form.get("type").map(|s| s.as_str()) == Some("page") {
        "page"
    } else {
        "post"
    };
    let need_rank = if kind == "page" { "editor" } else { "contributor" };
    let Ok((site, user, _)) = need(&state, &headers, need_rank).await else {
        return redirect_login();
    };
    let many = repeated(&body);
    let cid = form.get("cid").and_then(|s| s.parse().ok()).unwrap_or(0);
    let existing = if cid > 0 {
        state.db.content_by_id(cid).await.ok().flatten()
    } else {
        None
    };
    let own = state.db.user_options(user.uid).await.unwrap_or_default();
    let mut text = form.get("text").cloned().unwrap_or_default();
    if form.get("markdown").is_some_and(|v| v != "0")
        && option_flag(&own, "markdown", false)
        && !text.starts_with("<!--markdown-->")
    {
        text = format!("<!--markdown-->{text}");
    }
    let created = form
        .get("date")
        .and_then(|s| parse_local(s, site.timezone()))
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let categories: Vec<_> = many
        .get("category[]")
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|n| n.parse::<i64>().ok())
        .filter_map(|mid| site.categories.iter().find(|c| c.mid == mid).cloned())
        .collect();
    let mut fields = HashMap::new();
    for field in parse_fields(&many) {
        if !crate::write::field_name_ok(&field.name) {
            continue;
        }
        let str_value = if field.kind == "json" {
            serde_json::from_str::<serde_json::Value>(
                &serde_json::to_string(&field.value).unwrap_or_default(),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| field.value.clone())
        } else if field.kind == "int" || field.kind == "float" {
            String::new()
        } else {
            field.value.clone()
        };
        fields.insert(
            field.name,
            FieldValue {
                str_value,
                int_value: if field.kind == "int" {
                    field.value.trim().parse().unwrap_or(0)
                } else {
                    0
                },
            },
        );
    }
    let post = Post {
        cid,
        title: {
            let title = form.get("title").cloned().unwrap_or_default();
            if title.trim().is_empty() {
                "未命名文档".into()
            } else {
                title
            }
        },
        slug: form.get("slug").cloned().unwrap_or_default(),
        created,
        modified: chrono::Utc::now().timestamp(),
        text,
        sort_order: existing.as_ref().map(|p| p.sort_order).unwrap_or(0),
        author_id: user.uid,
        template: String::new(),
        kind: kind.into(),
        status: "publish".into(),
        password: String::new(),
        comments_num: 0,
        allow_comment: true,
        allow_ping: true,
        allow_feed: true,
        parent: 0,
        author_screen: user.label(),
        author_name: user.name.clone(),
        author_mail: user.mail.clone(),
        author_url: user.url.clone(),
        categories,
        tags: Vec::new(),
        fields,
    };
    let now = chrono::Utc::now().timestamp();
    let recent_posts = state
        .db
        .list_posts(&ListFilter::Index, 1, site.int("postsListSize", 10).max(1), now, false)
        .await
        .map(|(rows, _)| rows)
        .unwrap_or_default();
    let mut archives = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Ok(times) = state.db.published_times(now).await {
        for ts in times {
            let (year, month, _) = text::ymd(ts, site.timezone());
            if seen.insert((year, month)) {
                archives.push((year, month, ts));
            }
        }
    }
    let recent = state.db.recent_comments(5, now).await.unwrap_or_default();
    let chrome = Chrome {
        cfg: &state.cfg,
        site: &site,
        theme_ready: crate::themes::style_ready(&state.cfg, &site.theme_name()),
        recent_posts: &recent_posts,
        archives: &archives,
        recent: &recent,
        now,
        cookies: HashMap::new(),
        viewer_name: None,
        viewer_uid: 0,
        viewer_auth: "",
        page_url: "",
    };
    let view = SingleView {
        post: &post,
        locked: false,
        password_error: "",
        comment_error: "",
        comments: &[],
        newer: None,
        older: None,
        remember_author: "",
        remember_mail: "",
        remember_url: "",
        remember_text: "",
        reply_to: 0,
        comment_nav: "",
        ping_html: "",
        attachment_html: "",
    };
    match state.theme.render_single(&chrome, &view) {
        Ok(html) => html_ok(html),
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "预览失败"),
    }
}

async fn profile_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "subscriber").await else {
        return redirect_login();
    };
    render_profile(&state, &site, &user, &q).await
}

async fn profile_save(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "subscriber").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    let many = repeated(&body);
    let action = form.get("do").map(|s| s.as_str()).unwrap_or("");
    match action {
        "profile" => save_profile_form(&state, &user, &form).await,
        "options" if user.pass("contributor") => save_profile_options(&state, &user, &form, &many).await,
        "password" => save_profile_password(&state, &user, &form).await,
        _ => redirect("/admin/profile.php"),
    }
}

async fn save_profile_form(state: &AppState, user: &SessionUser, form: &HashMap<String, String>) -> Response {
    let mut screen = form.get("screenName").cloned().unwrap_or_default();
    let url = form.get("url").cloned().unwrap_or_default().trim().to_string();
    let mail = form.get("mail").cloned().unwrap_or_default().trim().to_string();
    let mut errors = Vec::new();
    if !screen.is_empty() {
        if state.db.screen_taken(user.uid, &screen).await.unwrap_or(false) {
            errors.push("昵称已经存在".to_string());
        } else if !auth::xss_ok(&screen) {
            errors.push("请不要在昵称中使用特殊字符".to_string());
        }
    } else {
        screen = user.name.clone();
    }
    if !url.is_empty() && !auth::url_ok(&url) {
        errors.push("个人主页地址格式错误".to_string());
    }
    if mail.is_empty() {
        errors.push("必须填写电子邮箱".to_string());
    } else if state.db.mail_taken(user.uid, &mail).await.unwrap_or(false) {
        errors.push("电子邮箱地址已经存在".to_string());
    } else if !auth::email_ok(&mail) {
        errors.push("电子邮箱格式错误".to_string());
    }
    if !errors.is_empty() {
        return profile_back(None, &errors);
    }
    if state
        .db
        .update_own_profile(user.uid, &screen, &mail, &url)
        .await
        .is_err()
    {
        return profile_back(None, &["保存失败".into()]);
    }
    profile_back(Some("您的档案已经更新"), &[])
}

async fn save_profile_options(
    state: &AppState,
    user: &SessionUser,
    form: &HashMap<String, String>,
    many: &HashMap<String, Vec<String>>,
) -> Response {
    let on = |key: &str| if form.get(key).is_some_and(|v| v != "0") { "1" } else { "0" };
    let allows = many
        .get("defaultAllow[]")
        .or_else(|| many.get("defaultAllow"))
        .cloned()
        .unwrap_or_default();
    let allow = |name: &str| if allows.iter().any(|v| v == name) { "1" } else { "0" };
    let pairs = [
        ("markdown", on("markdown")),
        ("xmlrpcMarkdown", on("xmlrpcMarkdown")),
        ("autoSave", on("autoSave")),
        ("defaultAllowComment", allow("comment")),
        ("defaultAllowPing", allow("ping")),
        ("defaultAllowFeed", allow("feed")),
    ];
    for (name, value) in pairs {
        if state.db.set_user_option(user.uid, name, value).await.is_err() {
            return profile_back(None, &["保存失败".into()]);
        }
    }
    profile_back(Some("设置已经保存"), &[])
}

async fn save_profile_password(state: &AppState, user: &SessionUser, form: &HashMap<String, String>) -> Response {
    let password = form.get("password").cloned().unwrap_or_default();
    let confirm = form.get("confirm").cloned().unwrap_or_default();
    let mut errors = Vec::new();
    if password.is_empty() {
        errors.push("必须填写密码".to_string());
    } else if password.chars().count() < 6 {
        errors.push("为了保证账户安全, 请输入至少六位的密码".to_string());
    }
    if password != confirm {
        errors.push("两次输入的密码不一致".to_string());
    }
    if !errors.is_empty() {
        return profile_back(None, &errors);
    }
    let hashed = auth::hash_password(&password);
    if state.db.update_password(user.uid, &hashed).await.is_err() {
        return profile_back(None, &["保存失败".into()]);
    }
    profile_back(Some("密码已经成功修改"), &[])
}

fn profile_back(notice: Option<&str>, errors: &[String]) -> Response {
    if let Some(notice) = notice {
        redirect(&format!("/admin/profile.php?notice={}", urlencoding::encode(notice)))
    } else if errors.is_empty() {
        redirect("/admin/profile.php")
    } else {
        redirect(&format!(
            "/admin/profile.php?error={}",
            urlencoding::encode(&errors.join("\n"))
        ))
    }
}

async fn render_profile(state: &AppState, site: &Site, user: &SessionUser, q: &HashMap<String, String>) -> Response {
    let (posts, comments, categories, logged) = state.db.profile_stats(user.uid).await.unwrap_or((0, 0, 0, user.logged));
    let fresh = auth::user_by_uid(&state.db, user.uid).await.ok().flatten();
    let user = fresh.as_ref().unwrap_or(user);
    let own = state.db.user_options(user.uid).await.unwrap_or_default();
    let hash = auth::md5_hex(user.mail.trim().to_ascii_lowercase().as_bytes());
    let avatar = format!(
        "{}{hash}?s=220&d=mm&r=X",
        state.cfg.gravatar_prefix
    );
    let login_line = if logged > 0 {
        format!("<p>最后登录: {}</p>", html_escape(&date_word(logged, site.timezone())))
    } else {
        String::new()
    };
    let writing = if user.pass("contributor") {
        format!(
            r#"<br><section id="writing-option"><h3>撰写设置</h3>
<form method="post" action="/action/users-profile" name="options"><ul class="typecho-option">
<li><label class="typecho-label">使用 Markdown 语法编辑和解析内容</label>
{md}<p class="description">使用 <a href="https://daringfireball.net/projects/markdown/">Markdown</a> 语法能够使您的撰写过程更加简便直观.<br>此功能开启不会影响以前没有使用 Markdown 语法编辑的内容.</p></li>
<li><label class="typecho-label">在 XMLRPC 接口中使用 Markdown 语法</label>
{xml}<p class="description">对于完全支持 <a href="https://daringfireball.net/projects/markdown/">Markdown</a> 语法写作的离线编辑器, 打开此选项后将避免内容被转换为 HTML.</p></li>
<li><label class="typecho-label">自动保存</label>
{auto}<p class="description">自动保存功能可以更好地保护你的文章不会丢失.</p></li>
<li><label class="typecho-label">默认允许</label>
<span class="multiline"><input type="checkbox" name="defaultAllow[]" value="comment"{c1}> 可以被评论</span>
<span class="multiline"><input type="checkbox" name="defaultAllow[]" value="ping"{c2}> 可以被引用</span>
<span class="multiline"><input type="checkbox" name="defaultAllow[]" value="feed"{c3}> 出现在聚合中</span>
<p class="description">设置你经常使用的默认允许权限</p></li>
</ul>
<input type="hidden" name="do" value="options">
<ul class="typecho-option"><li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li></ul>
</form></section>"#,
            md = radios("markdown", &[("0", "关闭"), ("1", "打开")], if option_flag(&own, "markdown", false) { "1" } else { "0" }),
            xml = radios("xmlrpcMarkdown", &[("0", "关闭"), ("1", "打开")], if option_flag(&own, "xmlrpcMarkdown", false) { "1" } else { "0" }),
            auto = radios("autoSave", &[("0", "关闭"), ("1", "打开")], if option_flag(&own, "autoSave", false) { "1" } else { "0" }),
            c1 = if option_flag(&own, "defaultAllowComment", true) { " checked" } else { "" },
            c2 = if option_flag(&own, "defaultAllowPing", true) { " checked" } else { "" },
            c3 = if option_flag(&own, "defaultAllowFeed", true) { " checked" } else { "" },
        )
    } else {
        String::new()
    };
    let main = format!(
        r##"<div class="row typecho-page-main">{flash}
<div class="col-mb-12 col-tb-3">
<p><a href="https://gravatar.com/emails/" title="在 Gravatar 上修改头像"><img class="profile-avatar" src="{avatar}" alt="{screen}"></a></p>
<h2>{screen}</h2>
<p>{name}</p>
<p>目前有 <em>{posts}</em> 篇日志, 并有 <em>{comments}</em> 条关于你的评论在 <em>{categories}</em> 个分类中.</p>
{login_line}
</div>
<div class="col-mb-12 col-tb-6 col-tb-offset-1 typecho-content-panel" role="form">
<section><h3>个人资料</h3>
<form method="post" action="/action/users-profile" name="profile"><ul class="typecho-option">
<li><label class="typecho-label" for="screenName">昵称</label>
<input id="screenName" name="screenName" type="text" class="w-100" value="{screen}">
<p class="description">用户昵称可以与用户名不同, 用于前台显示.<br>如果你将此项留空, 将默认使用用户名.</p></li>
<li><label class="typecho-label" for="url">个人主页地址</label>
<input id="url" name="url" type="text" class="w-100" value="{url}">
<p class="description">此用户的个人主页地址, 请用 <code>http://</code> 开头.</p></li>
<li><label class="typecho-label" for="mail">邮件地址 *</label>
<input id="mail" name="mail" type="text" class="w-100" value="{mail}">
<p class="description">电子邮箱地址将作为此用户的主要联系方式.<br>请不要与系统中现有的电子邮箱地址重复.</p></li>
</ul>
<input type="hidden" name="do" value="profile">
<ul class="typecho-option"><li class="typecho-option-submit"><button type="submit" class="btn primary">更新我的档案</button></li></ul>
</form></section>
{writing}
<br>
<section id="change-password"><h3>密码修改</h3>
<form method="post" action="/action/users-profile" name="password"><ul class="typecho-option">
<li><label class="typecho-label" for="password">用户密码</label>
<input id="password" name="password" type="password" class="w-60" autocomplete="new-password">
<p class="description">为此用户分配一个密码.<br>建议使用特殊字符与字母、数字的混编样式,以增加系统安全性.</p></li>
<li><label class="typecho-label" for="confirm">用户密码确认</label>
<input id="confirm" name="confirm" type="password" class="w-60" autocomplete="new-password">
<p class="description">请确认你的密码, 与上面输入的密码保持一致.</p></li>
</ul>
<input type="hidden" name="do" value="password">
<ul class="typecho-option"><li class="typecho-option-submit"><button type="submit" class="btn primary">更新密码</button></li></ul>
</form></section>
{personal}
</div></div>"##,
        flash = flash_html(q),
        avatar = html_escape(&avatar),
        screen = html_escape(&user.label()),
        name = html_escape(&user.name),
        url = html_escape(&user.url),
        mail = html_escape(&user.mail),
        personal = plugins::personal_html(site),
    );
    layout(site, user, "profile", "个人设置", None, "container", &main)
}

fn flash_html(q: &HashMap<String, String>) -> String {
    if let Some(error) = q.get("error").filter(|s| !s.is_empty()) {
        let items = error
            .split('\n')
            .filter(|s| !s.is_empty())
            .map(|s| format!("<li>{}</li>", html_escape(s)))
            .collect::<String>();
        return format!(r#"<div class="message error"><ul>{items}</ul></div>"#);
    }
    if let Some(notice) = q.get("notice").filter(|s| !s.is_empty()) {
        return format!(
            r#"<div class="message success"><ul><li>{}</li></ul></div>"#,
            html_escape(notice)
        );
    }
    String::new()
}

fn option_flag(map: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match map.get(key).map(|s| s.as_str()).unwrap_or("") {
        "" => default,
        "0" | "false" | "off" | "no" => false,
        _ => true,
    }
}

fn custom_fields_html(rows: &[FieldRow]) -> String {
    let fold = if rows.is_empty() { " fold" } else { "" };
    let mut body = String::new();
    if rows.is_empty() {
        body.push_str(&field_row("", "str", "", true));
    } else {
        for row in rows {
            let value = match row.kind.as_str() {
                "int" => row.int_value.to_string(),
                "float" => row.float_text.clone(),
                _ => row.str_value.clone(),
            };
            body.push_str(&field_row(&row.name, &row.kind, &value, false));
        }
    }
    format!(
        r###"<section id="custom-field" class="typecho-post-option{fold}"><label id="custom-field-expand" class="typecho-label"><a href="##"><i class="i-caret-right"></i> 自定义字段</a></label>
<table class="typecho-list-table mono"><colgroup><col width="20%"><col width="15%"><col width="55%"><col width="10%"></colgroup>
<tbody>{body}</tbody></table>
<div class="description clearfix"><button type="button" class="btn btn-xs operate-add">+添加字段</button>
自定义字段可以扩展你的模板功能, 使用方法参见 <a href="https://docs.typecho.org/help/custom-fields">帮助文档</a></div></section>"###
    )
}

fn field_row(name: &str, kind: &str, value: &str, blank: bool) -> String {
    let opt = |value_name: &str, label: &str| {
        let sel = if kind == value_name { " selected" } else { "" };
        format!("<option value=\"{value_name}\"{sel}>{label}</option>")
    };
    let json = if kind == "json" { opt("json", "JSON 结构") } else { String::new() };
    let placeholder = if blank {
        (" placeholder=\"字段名称\"", " placeholder=\"字段值\"")
    } else {
        ("", "")
    };
    format!(
        r#"<tr><td><label class="sr-only">字段名称</label><input type="text" name="fieldNames[]" value="{name}" class="text-s w-100"{ph_name}></td>
<td><label class="sr-only">字段类型</label><select name="fieldTypes[]">{str}{int}{float}{json}</select></td>
<td><label class="sr-only">字段值</label><textarea name="fieldValues[]" class="text-s w-100" rows="2"{ph_value}>{value}</textarea></td>
<td><button type="button" class="btn btn-xs">删除</button></td></tr>"#,
        name = html_escape(name),
        value = html_escape(value),
        str = opt("str", "字符"),
        int = opt("int", "整数"),
        float = opt("float", "小数"),
        ph_name = placeholder.0,
        ph_value = placeholder.1,
    )
}

fn parse_fields(many: &HashMap<String, Vec<String>>) -> Vec<FieldInput> {
    let names = many.get("fieldNames[]").cloned().unwrap_or_default();
    let types = many.get("fieldTypes[]").cloned().unwrap_or_default();
    let values = many.get("fieldValues[]").cloned().unwrap_or_default();
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        out.push(FieldInput {
            name,
            kind: types.get(i).cloned().unwrap_or_else(|| "str".into()),
            value: values.get(i).cloned().unwrap_or_default(),
        });
    }
    out
}

async fn apply_comment(state: &AppState, coid: i64, act: &str) {
    if act == "delete" {
        let _ = state.db.delete_comment(coid).await;
    } else if act == "approved" || act == "spam" || act == "waiting" {
        let _ = state.db.set_comment_status(coid, act).await;
    }
}

async fn media(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let rows = state.db.list_contents("attachment", 300).await.unwrap_or_default();
    let keywords = q.get("keywords").map(|s| s.as_str()).unwrap_or("");
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|p| user.pass("editor") || p.author_id == user.uid)
        .filter(|p| keywords.is_empty() || title_hit(&p.title, keywords))
        .collect();
    let mut body_rows = String::new();
    if rows.is_empty() {
        body_rows.push_str("<tr><td colspan=\"6\"><h6 class=\"typecho-list-table-title\">没有任何文件</h6></td></tr>");
    }
    for row in &rows {
        let file = crate::write::parse_attachment(&row.text);
        let mime = file.as_ref().map(|f| mime_icon(&f.mime)).unwrap_or_else(|| "unknown".into());
        let parent_cell = attachment_parent_cell(&state, row.parent).await;
        let link = links::content_path(&site, row);
        body_rows.push_str(&format!(
            r#"<tr id="attachment-{cid}"><td class="kit-hidden-mb"><input type="checkbox" value="{cid}" name="cid[]"></td>
<td class="kit-hidden-mb"><a href="/admin/manage-comments.php?cid={cid}" class="balloon-button size-{size}">{comments}</a></td>
<td><i class="mime-{mime}"></i> <a href="/admin/media.php?cid={cid}">{title}</a> <a href="{link}" title="浏览 {title}"><i class="i-exlink"></i></a></td>
<td class="kit-hidden-mb">{author}</td>
<td class="kit-hidden-mb">{parent_cell}</td>
<td>{date}</td></tr>"#,
            cid = row.cid,
            size = count_size(row.comments_num),
            comments = row.comments_num,
            title = html_escape(&row.title),
            link = html_escape(&link),
            author = html_escape(&row.display_author()),
            date = html_escape(&date_word(row.created, site.timezone())),
        ));
    }
    let cancel = if keywords.is_empty() {
        String::new()
    } else {
        "<a href=\"/admin/manage-medias.php\">&laquo; 取消筛选</a>".into()
    };
    let main = format!(
        r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12">
{flash}
<div class="typecho-list-operate clearfix"><form method="get"><div class="operate">
<label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button"><i class="sr-only">操作</i>选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu"><li><a lang="你确认要删除这些文件吗?" href="/action/contents-attachment-edit?do=delete">删除</a></li></ul>
</div>
<button class="btn btn-s btn-warn btn-operate" type="button" href="/action/contents-attachment-edit?do=clear" lang="您确认要清理未归档的文件吗?">清理未归档文件</button>
</div><div class="search" role="search">{cancel}
<input type="text" class="text-s" placeholder="请输入关键字" name="keywords" value="{kw}">
<button type="submit" class="btn btn-s">筛选</button></div></form></div>
<form method="post" name="manage_medias" class="operate-form"><div class="typecho-table-wrap"><table class="typecho-list-table">
<colgroup><col width="20" class="kit-hidden-mb"><col width="6%" class="kit-hidden-mb"><col width="30%"><col width="" class="kit-hidden-mb"><col width="30%" class="kit-hidden-mb"><col width="16%"></colgroup>
<thead><tr><th class="kit-hidden-mb"></th><th class="kit-hidden-mb"></th><th>文件名</th><th class="kit-hidden-mb">上传者</th><th class="kit-hidden-mb">所属文章</th><th>发布日期</th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form>
</div></div>"#,
        flash = flash_html(&q),
        kw = html_escape(keywords),
    );
    layout(&site, &user, "media", "管理文件", None, "container", &main)
}

async fn media_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Response {
    if !action_allowed(&state, &headers, &q, "").await {
        return go_back(&headers);
    }
    if q.get("do").map(|s| s.as_str()) == Some("modify") {
        let cid = q.get("cid").and_then(|s| s.parse().ok()).unwrap_or(0);
        return media_replace(&state, &headers, cid, multipart).await;
    }
    let Ok((site, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let mut parent = 0i64;
    let mut ajax = false;
    let mut name = String::from("file.bin");
    let mut bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name == "parent" || field_name == "cid" {
            parent = field.text().await.unwrap_or_default().trim().parse().unwrap_or(0);
        } else if field_name == "ajax" {
            ajax = field.text().await.unwrap_or_default() != "0";
        } else if field_name == "file" {
            if let Some(n) = field.file_name() {
                name = n.to_string();
            }
            bytes = field.bytes().await.unwrap_or_default().to_vec();
        }
    }
    if bytes.is_empty() {
        return if ajax {
            json_raw("false")
        } else {
            plain(StatusCode::BAD_REQUEST, "没有文件")
        };
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
    let now = chrono::Utc::now().timestamp();
    let local = now + site.timezone();
    let stamp = chrono::DateTime::from_timestamp(local, 0);
    let folder = stamp.map(|d| format!("{}/{}", d.format("%Y"), d.format("%m"))).unwrap_or_else(|| "0000/00".into());
    let file_name = format!("{}-{safe}", &auth::md5_hex(&bytes)[..12]);
    let root = state.cfg.uploads_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("usr/uploads"));
    let dir = root.join(&folder);
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(dir.join(&file_name), &bytes).is_err() {
        return if ajax {
            json_raw("false")
        } else {
            plain(StatusCode::INTERNAL_SERVER_ERROR, "写入上传目录失败")
        };
    }
    let rel = format!("usr/uploads/{folder}/{file_name}");
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    if parent > 0 {
        match state.db.content_by_id(parent).await {
            Ok(Some(post)) if post.kind != "attachment" && (user.pass("editor") || post.author_id == user.uid) => {}
            _ => parent = 0,
        }
    }
    let text = crate::write::attachment_text(&safe, &rel, bytes.len() as i64, &ext, mime);
    let saved = state
        .db
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
            parent,
            sort_order: 0,
            categories: Vec::new(),
            tags: Vec::new(),
        })
        .await;
    let Ok(cid) = saved else {
        return if ajax {
            json_raw("false")
        } else {
            plain(StatusCode::INTERNAL_SERVER_ERROR, "保存附件失败")
        };
    };
    if ajax {
        let url = format!("/{rel}");
        return json_raw(&upload_payload(&url, cid, &safe, &ext, bytes.len() as i64));
    }
    redirect("/admin/manage-medias.php")
}

async fn media_edit(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>, Query(q): Query<HashMap<String, String>>) -> Response {
    media_form(&state, &headers, cid, &q, &HashMap::new()).await
}

async fn media_update(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(cid): Path<i64>, body: String) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "contributor").await else {
        return redirect_login();
    };
    let Some(post) = load_attachment(&state, cid, &user).await else {
        return plain(StatusCode::NOT_FOUND, "文件不存在");
    };
    let form = pairs(&body);
    let name = form.get("name").cloned().unwrap_or_default();
    let slug_input = form.get("slug").cloned().unwrap_or_default();
    let mut errors = Vec::new();
    if name.trim().is_empty() {
        errors.push("必须填写文件标题".to_string());
    } else if slug_input.trim().is_empty() {
        let generated = crate::write::typecho_slug(name.trim());
        if generated.is_empty() || state.db.attachment_slug_taken(&generated, cid).await.unwrap_or(false) {
            errors.push("文件标题无法被转换为缩略名".into());
        }
    }
    let slug = crate::write::typecho_slug(if slug_input.trim().is_empty() { name.trim() } else { slug_input.trim() });
    if !slug.is_empty() && state.db.attachment_slug_taken(&slug, cid).await.unwrap_or(false) {
        if !(slug_input.trim().is_empty() && errors.iter().any(|e| e.contains("无法被转换为缩略名"))) {
            errors.push("缩略名已经存在".into());
        }
    }
    if !errors.is_empty() {
        let mut q = HashMap::new();
        q.insert("error".into(), errors.join("\n"));
        return media_form(&state, &headers, cid, &q, &form).await;
    }
    let mut file = crate::write::parse_attachment(&post.text).unwrap_or(crate::write::AttachmentFile {
        name: name.clone(),
        path: String::new(),
        size: 0,
        ext: String::new(),
        mime: "application/octet-stream".into(),
        description: String::new(),
    });
    file.description = form.get("description").cloned().unwrap_or_default();
    let text = crate::write::serialize_attachment(&file);
    if state
        .db
        .update_attachment_fields(cid, name.trim(), &slug, &text)
        .await
        .is_err()
    {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "保存文件失败");
    }
    let notice = if post.status == "publish" {
        format!("文件 {} 已经被更新", name.trim())
    } else {
        format!("未归档文件 {} 已经被更新", name.trim())
    };
    notice_to("/admin/manage-medias.php", &notice)
}

async fn media_replace(state: &AppState, headers: &HeaderMap, cid: i64, mut multipart: Multipart) -> Response {
    let Ok((_, user, _)) = need(state, headers, "contributor").await else {
        return redirect_login();
    };
    let Some(post) = load_attachment(&state, cid, &user).await else {
        return plain(StatusCode::NOT_FOUND, "文件不存在");
    };
    let Some(mut file) = crate::write::parse_attachment(&post.text) else {
        return error_to(&format!("/admin/media.php?cid={cid}"), "上传出现错误");
    };
    let mut name = String::new();
    let mut bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            if let Some(n) = field.file_name() {
                name = n.to_string();
            }
            bytes = field.bytes().await.unwrap_or_default().to_vec();
        }
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if bytes.is_empty() || ext != file.ext.to_ascii_lowercase() {
        let message = if ext != file.ext.to_ascii_lowercase() && !bytes.is_empty() {
            "文件扩展名不被支持"
        } else {
            "上传出现错误"
        };
        return error_to(&format!("/admin/media.php?cid={cid}"), message);
    }
    let path = upload_path(&state, &file.path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&path, &bytes).is_err() {
        return error_to(&format!("/admin/media.php?cid={cid}"), "上传出现错误");
    }
    file.size = bytes.len() as i64;
    let text = crate::write::serialize_attachment(&file);
    let _ = state
        .db
        .update_attachment_fields(cid, &post.title, &post.slug, &text)
        .await;
    notice_to(&format!("/admin/media.php?cid={cid}"), &format!("文件 {} 已经替换", file.name))
}

async fn media_form(
    state: &AppState,
    headers: &HeaderMap,
    cid: i64,
    q: &HashMap<String, String>,
    posted: &HashMap<String, String>,
) -> Response {
    let Ok((site, user, _)) = need(state, headers, "contributor").await else {
        return redirect_login();
    };
    let Some(post) = load_attachment(state, cid, &user).await else {
        return plain(StatusCode::NOT_FOUND, "文件不存在");
    };
    let file = crate::write::parse_attachment(&post.text);
    let name = posted.get("name").cloned().unwrap_or_else(|| post.title.clone());
    let slug = posted.get("slug").cloned().unwrap_or_else(|| post.slug.clone());
    let description = posted
        .get("description")
        .cloned()
        .or_else(|| file.as_ref().map(|f| f.description.clone()))
        .unwrap_or_default();
    let url = file.as_ref().map(|f| format!("/{}", f.path.trim_start_matches('/'))).unwrap_or_default();
    let mime = file.as_ref().map(|f| mime_icon(&f.mime)).unwrap_or_else(|| "unknown".into());
    let file_name = file.as_ref().map(|f| f.name.clone()).unwrap_or_else(|| post.title.clone());
    let kb = file.as_ref().map(|f| kb_of(f.size)).unwrap_or(0);
    let image = file
        .as_ref()
        .filter(|f| crate::write::attachment_is_image(&f.ext))
        .map(|_| {
            format!(
                "<p><img src=\"{url}\" alt=\"{name}\" class=\"typecho-attachment-photo\"></p>",
                url = html_escape(&url),
                name = html_escape(&file_name)
            )
        })
        .unwrap_or_default();
    let delete_lang = format!("你确认删除文件 {file_name} 吗?");
    let main = format!(
        r#"<div class="row typecho-page-main">
<div class="col-mb-12 col-tb-8" role="main">
{flash}
{image}
<p><i class="mime-{mime}"></i> <a href=""><strong>{file_name}</strong></a> <span>{kb} Kb</span></p>
<p><input id="attachment-url" type="text" class="mono w-100" value="{url}" readonly></p>
<div id="upload-panel" class="p">
<form method="post" action="/action/upload?do=modify&cid={cid}" enctype="multipart/form-data">
<input type="hidden" name="do" value="modify">
<div class="upload-area" draggable="true">拖放文件到这里<br>或者 <label class="upload-file">选择文件上传<input type="file" name="file" class="hidden"></label></div>
</form>
<ul id="file-list"></ul>
</div>
</div>
<div class="col-mb-12 col-tb-4 edit-media" role="form">
<form method="post" action="/action/contents-attachment-edit" enctype="application/x-www-form-urlencoded">
<ul class="typecho-option"><li><label class="typecho-label" for="name">标题 *</label>
<input id="name" name="name" type="text" class="text" value="{name}"></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="slug">缩略名</label>
<input id="slug" name="slug" type="text" class="text" value="{slug}">
<p class="description">文件缩略名用于创建友好的链接形式,建议使用字母,数字,下划线和横杠.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="description">描述</label>
<textarea id="description" name="description">{description}</textarea>
<p class="description">此文字用于描述文件,在有的主题中它会被显示.</p></li></ul>
<input type="hidden" name="do" value="update">
<input type="hidden" name="cid" value="{cid}">
<ul class="typecho-option typecho-option-submit"><li>
<button type="submit" class="btn primary">提交修改</button>
<button type="submit" class="operate-delete" form="delete-attachment" lang="{delete_lang}">删除文件</button>
</li></ul>
</form>
<form id="delete-attachment" method="post" action="/action/contents-attachment-edit?do=delete&cid={cid}"></form>
</div></div>"#,
        flash = flash_html(q),
        file_name = html_escape(&file_name),
        url = html_escape(&url),
        name = html_escape(&name),
        slug = html_escape(&slug),
        description = html_escape(&description),
        delete_lang = html_escape(&delete_lang),
    );
    layout(&site, &user, "media", &format!("编辑 {}", post.title), None, "container", &main)
}

async fn load_attachment(state: &AppState, cid: i64, user: &SessionUser) -> Option<Post> {
    let post = state.db.content_by_id(cid).await.ok().flatten()?;
    if post.kind != "attachment" {
        return None;
    }
    if !user.pass("editor") && post.author_id != user.uid {
        return None;
    }
    Some(post)
}

async fn attachment_parent_cell(state: &AppState, parent: i64) -> String {
    if parent <= 0 {
        return "<span class=\"description\">未归档</span>".into();
    }
    let Ok(Some(post)) = state.db.content_by_id(parent).await else {
        return "<span class=\"description\">未归档</span>".into();
    };
    if post.kind == "attachment" {
        return "<span class=\"description\">未归档</span>".into();
    }
    let kind = if post.kind == "page" { "pages" } else { "posts" };
    format!(
        "<a href=\"/admin/write-post.php?cid={cid}\">{title}</a>",
        cid = post.cid,
        title = html_escape(&post.title)
    )
}

async fn attachment_tab(state: &AppState, user: &SessionUser, cid: Option<i64>) -> (String, usize) {
    let rows = state.db.list_contents("attachment", 300).await.unwrap_or_default();
    let files: Vec<_> = if let Some(cid) = cid {
        let mut rows: Vec<_> = rows.into_iter().filter(|p| p.parent == cid).collect();
        rows.sort_by_key(|p| p.created);
        rows
    } else {
        let mut rows: Vec<_> = rows
            .into_iter()
            .filter(|p| p.parent == 0 && p.author_id == user.uid)
            .collect();
        rows.sort_by_key(|p| std::cmp::Reverse(p.created));
        rows
    };
    let mut items = String::new();
    let mut count = 0usize;
    for post in &files {
        let Some(file) = crate::write::parse_attachment(&post.text) else {
            continue;
        };
        count += 1;
        let url = format!("/{}", file.path.trim_start_matches('/'));
        let image = if crate::write::attachment_is_image(&file.ext) { "1" } else { "0" };
        items.push_str(&format!(
            r##"<li data-cid="{cid}" data-url="{url}" data-image="{image}"><input type="hidden" name="attachment[]" value="{cid}"><a class="insert" title="点击插入文件" href="#">{title}</a><div class="info">{kb} Kb <a class="file" target="_blank" href="/admin/media.php?cid={cid}" title="编辑"><i class="i-edit"></i></a> <a href="#" class="delete" title="删除"><i class="i-delete"></i></a></div></li>"##,
            cid = post.cid,
            url = html_escape(&url),
            title = html_escape(&post.title),
            kb = kb_of(file.size),
        ));
    }
    let _ = files;
    let html = format!(
        r##"<div id="upload-panel" class="p"><div class="upload-area" draggable="true">拖放文件到这里<br>或者 <a href="#" class="upload-file">选择文件上传</a></div><ul id="file-list">{items}</ul></div>"##
    );
    (html, count)
}

fn upload_payload(url: &str, cid: i64, title: &str, ext: &str, size: i64) -> String {
    serde_json::json!([
        url,
        {
            "cid": cid,
            "title": title,
            "type": ext,
            "size": size,
            "bytes": format!("{} Kb", kb_of(size)),
            "isImage": crate::write::attachment_is_image(ext),
            "url": url,
            "permalink": format!("/attachment/{cid}/"),
        }
    ])
    .to_string()
}

fn kb_of(size: i64) -> i64 {
    if size <= 0 { 0 } else { (size + 1023) / 1024 }
}

fn upload_path(state: &AppState, stored: &str) -> std::path::PathBuf {
    let rel = stored.trim_start_matches('/').trim_start_matches("usr/uploads/");
    let root = state
        .cfg
        .uploads_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("usr/uploads"));
    root.join(rel)
}

fn remove_upload_file(state: &AppState, text: &str) {
    if let Some(file) = crate::write::parse_attachment(text) {
        let _ = std::fs::remove_file(upload_path(state, &file.path));
    }
}

fn mime_icon(mime: &str) -> String {
    let mut parts = mime.split('/');
    let kind = parts.next().unwrap_or("");
    let stream = parts.next().unwrap_or("");
    if stream.is_empty() || !matches!(kind, "image" | "video" | "audio" | "text" | "application") {
        return "unknown".into();
    }
    if matches!(stream, "msword" | "msaccess" | "ms-powerpoint") || stream.starts_with("vnd.") {
        return "office".into();
    }
    if stream.contains("html") || stream.contains("xml") || stream.contains("wml") {
        return "html".into();
    }
    if stream.contains("compressed") || stream.contains("zip") || stream.contains("tar") {
        return "archive".into();
    }
    if kind == "text" && stream.starts_with("x-") {
        return "script".into();
    }
    kind.into()
}

fn json_raw(body: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "json"))
}

async fn users(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let keywords = q.get("keywords").map(|s| s.as_str()).unwrap_or("");
    let rows = state.db.list_users().await.unwrap_or_default();
    let counts = state.db.post_counts().await.unwrap_or_default();
    let mut body_rows = String::new();
    let shown: Vec<_> = rows
        .iter()
        .filter(|r| keywords.is_empty() || title_hit(&r.name, keywords) || title_hit(&r.screen, keywords))
        .collect();
    if shown.is_empty() {
        body_rows.push_str("<tr><td colspan=\"6\"><h6 class=\"typecho-list-table-title\">没有任何用户</h6></td></tr>");
    }
    for row in shown {
        let posts = counts.get(&row.uid).copied().unwrap_or(0);
        let size = split_by_count(posts, &[1, 10, 20, 50, 100]);
        let mail = if row.mail.is_empty() {
            "暂无".to_string()
        } else {
            format!("<a href=\"mailto:{mail}\">{mail}</a>", mail = html_escape(&row.mail))
        };
        let link = links::author_path(&site, row.uid);
        body_rows.push_str(&format!(
            r#"<tr id="user-{uid}"><td class="kit-hidden-mb"><input type="checkbox" value="{uid}" name="uid[]"></td>
<td class="kit-hidden-mb"><a href="/admin/manage-posts.php?__typecho_all_posts=off&amp;uid={uid}" class="balloon-button left size-{size}">{posts}</a></td>
<td><a href="/admin/user.php?uid={uid}">{name}</a> <a href="{link}" title="浏览 {screen}"><i class="i-exlink"></i></a></td>
<td class="kit-hidden-mb">{screen}</td>
<td class="kit-hidden-mb">{mail}</td>
<td>{group}</td></tr>"#,
            uid = row.uid,
            name = html_escape(&row.name),
            screen = html_escape(&row.label()),
            link = html_escape(&link),
            group = group_label(&row.group),
        ));
    }
    let main = format!(
        r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12 typecho-list">
{flash}
{operate_top}
<form method="post" name="manage_users" class="operate-form">
<div class="typecho-table-wrap"><table class="typecho-list-table">
<colgroup><col width="20" class="kit-hidden-mb"><col width="6%" class="kit-hidden-mb"><col width="30%"><col width="" class="kit-hidden-mb"><col width="25%" class="kit-hidden-mb"><col width="15%"></colgroup>
<thead><tr><th class="kit-hidden-mb"></th><th class="kit-hidden-mb"></th><th>用户名</th><th class="kit-hidden-mb">昵称</th><th class="kit-hidden-mb">电子邮件</th><th>用户组</th></tr></thead>
<tbody>{body_rows}</tbody></table></div></form>
{operate_bottom}
</div></div>"#,
        flash = flash_html(&q),
        operate_top = users_operate(true, keywords),
        operate_bottom = users_operate(false, ""),
    );
    layout(&site, &user, "users", "管理用户", Some("/admin/user.php"), "container", &main)
}

fn users_operate(with_search: bool, keywords: &str) -> String {
    let search = if with_search {
        let cancel = if keywords.is_empty() {
            String::new()
        } else {
            "<a href=\"/admin/manage-users.php\">&laquo; 取消筛选</a>".to_string()
        };
        format!(
            r#"<div class="search" role="search">{cancel}<input type="text" class="text-s" placeholder="请输入关键字" value="{kw}" name="keywords"><button type="submit" class="btn btn-s">筛选</button></div>"#,
            kw = html_escape(keywords)
        )
    } else {
        String::new()
    };
    format!(
        r#"<div class="typecho-list-operate clearfix"><form method="get"><div class="operate">
<label><i class="sr-only">全选</i><input type="checkbox" class="typecho-table-select-all"></label>
<div class="btn-group btn-drop"><button class="btn dropdown-toggle btn-s" type="button"><i class="sr-only">操作</i>选中项 <i class="i-caret-down"></i></button>
<ul class="dropdown-menu"><li><a lang="你确认要删除这些用户吗?" href="/action/users-edit?do=delete">删除</a></li></ul></div>
</div>{search}</form></div>"#
    )
}

async fn user_php(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let uid = q.get("uid").and_then(|s| s.parse::<i64>().ok()).filter(|n| *n > 0);
    user_form(&state, &headers, uid, uid.is_some(), &q, &HashMap::new()).await
}

async fn user_form(
    state: &AppState,
    headers: &HeaderMap,
    uid: Option<i64>,
    editing: bool,
    q: &HashMap<String, String>,
    posted: &HashMap<String, String>,
) -> Response {
    let Ok((site, actor, _)) = need(state, headers, "administrator").await else {
        return redirect_login();
    };
    let current = if let Some(uid) = uid {
        match state.db.list_users().await.unwrap_or_default().into_iter().find(|r| r.uid == uid) {
            Some(user) => Some(user),
            None if posted.is_empty() => return plain(StatusCode::NOT_FOUND, "用户不存在"),
            None => None,
        }
    } else {
        None
    };
    let posted_text = |key: &str| posted.get(key).cloned().unwrap_or_default();
    let name = if editing {
        current.as_ref().map(|u| u.name.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| posted_text("name"))
    } else {
        posted_text("name")
    };
    let screen = if posted.contains_key("screenName") || posted.contains_key("screen") {
        posted.get("screenName").cloned().or_else(|| posted.get("screen").cloned()).unwrap_or_default()
    } else {
        current.as_ref().map(|u| u.screen.clone()).unwrap_or_default()
    };
    let mail = if posted.contains_key("mail") {
        posted_text("mail")
    } else {
        current.as_ref().map(|u| u.mail.clone()).unwrap_or_default()
    };
    let url = if posted.contains_key("url") {
        posted_text("url")
    } else {
        current.as_ref().map(|u| u.url.clone()).unwrap_or_default()
    };
    let group = if posted.contains_key("group") {
        posted_text("group")
    } else {
        current.as_ref().map(|u| u.group.clone()).unwrap_or_else(|| "subscriber".into())
    };
    let (do_value, submit, password_label, confirm_label, title) = if editing {
        let title = if name.is_empty() {
            "编辑用户".to_string()
        } else {
            format!("编辑用户 {}", html_escape(&name))
        };
        ("update", "编辑用户", "用户密码", "用户密码确认", title)
    } else {
        ("insert", "增加用户", "用户密码 *", "用户密码确认 *", "新增用户".into())
    };
    let disabled = if editing { " disabled=\"disabled\"" } else { "" };
    let uid_value = uid.map(|n| n.to_string()).unwrap_or_default();
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-6 col-tb-offset-3">
{flash}
<form method="post" action="/action/users-edit" enctype="application/x-www-form-urlencoded">
<ul class="typecho-option"><li><label class="typecho-label" for="name">用户名 *</label>
<input id="name" name="name" type="text" class="text" value="{name}"{disabled}>
<p class="description">此用户名将作为用户登录时所用的名称.<br>请不要与系统中现有的用户名重复.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="mail">邮件地址 *</label>
<input id="mail" name="mail" type="text" class="text" value="{mail}">
<p class="description">电子邮箱地址将作为此用户的主要联系方式.<br>请不要与系统中现有的电子邮箱地址重复.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="screenName">用户昵称</label>
<input id="screenName" name="screenName" type="text" class="text" value="{screen}">
<p class="description">用户昵称可以与用户名不同, 用于前台显示.<br>如果你将此项留空, 将默认使用用户名.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="password">{password_label}</label>
<input id="password" name="password" type="password" class="w-60" autocomplete="new-password">
<p class="description">为此用户分配一个密码.<br>建议使用特殊字符与字母、数字的混编样式,以增加系统安全性.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="confirm">{confirm_label}</label>
<input id="confirm" name="confirm" type="password" class="w-60" autocomplete="new-password">
<p class="description">请确认你的密码, 与上面输入的密码保持一致.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="url">个人主页地址</label>
<input id="url" name="url" type="text" class="text" value="{url}">
<p class="description">此用户的个人主页地址, 请用 <code>http://</code> 开头.</p></li></ul>
<ul class="typecho-option"><li><label class="typecho-label" for="group">用户组</label>
<select id="group" name="group">{groups}</select>
<p class="description">不同的用户组拥有不同的权限.<br>具体的权限分配表请<a href="https://docs.typecho.org/develop/acl">参考这里</a>.</p></li></ul>
<input type="hidden" name="do" value="{do_value}">
<input type="hidden" name="uid" value="{uid_value}">
<ul class="typecho-option typecho-option-submit"><li><button type="submit" class="btn primary">{submit}</button></li></ul>
</form></div></div>"#,
        flash = flash_html(q),
        name = html_escape(&name),
        mail = html_escape(&mail),
        screen = html_escape(&screen),
        url = html_escape(&url),
        groups = group_options(&group),
    );
    layout(&site, &actor, "users", &title, None, "container", &main)
}

fn group_options(current: &str) -> String {
    let mut html = String::new();
    for (value, label) in [
        ("subscriber", "关注者"),
        ("contributor", "贡献者"),
        ("editor", "编辑"),
        ("administrator", "管理员"),
    ] {
        let sel = if value == current { " selected" } else { "" };
        html.push_str(&format!("<option value=\"{value}\"{sel}>{label}</option>"));
    }
    html
}

fn group_label(group: &str) -> &'static str {
    match group {
        "administrator" => "管理员",
        "editor" => "编辑",
        "contributor" => "贡献者",
        "subscriber" => "关注者",
        _ => "访问者",
    }
}

async fn user_commit(state: &AppState, headers: &HeaderMap, verb: &str, body: &str) -> Response {
    let Ok((_, _, _)) = need(state, headers, "administrator").await else {
        return redirect_login();
    };
    let form = pairs(body);
    let form_do = form.get("do").map(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or(verb);
    let uid = form.get("uid").and_then(|s| s.parse::<i64>().ok()).filter(|n| *n > 0);
    let updating = match form_do {
        "update" => true,
        "insert" => false,
        _ => uid.is_some(),
    };
    let rows = state.db.list_users().await.unwrap_or_default();
    let current = uid.and_then(|id| rows.iter().find(|r| r.uid == id).cloned());
    let name = if updating {
        current.as_ref().map(|u| u.name.clone()).unwrap_or_default()
    } else {
        form.get("name").cloned().unwrap_or_default().trim().to_string()
    };
    let mut screen = form.get("screenName").cloned().or_else(|| form.get("screen").cloned()).unwrap_or_default();
    screen = screen.trim().to_string();
    let mail = form.get("mail").cloned().unwrap_or_default().trim().to_string();
    let url = form.get("url").cloned().unwrap_or_default().trim().to_string();
    let password = form.get("password").cloned().unwrap_or_default();
    let confirm = form.get("confirm").cloned().unwrap_or_default();
    let except = if updating { uid.unwrap_or(0) } else { 0 };
    let mut errors = Vec::new();
    if !updating {
        if name.is_empty() {
            errors.push("必须填写用户名称".to_string());
        } else if !auth::xss_ok(&name) {
            errors.push("请不要在用户名中使用特殊字符".to_string());
        } else if state.db.login_name_taken(except, &name).await.unwrap_or(false) {
            errors.push("用户名已经存在".to_string());
        }
    }
    if mail.is_empty() {
        errors.push("必须填写电子邮箱".to_string());
    } else {
        if state.db.mail_taken(except, &mail).await.unwrap_or(false) {
            errors.push("电子邮箱地址已经存在".to_string());
        }
        if !auth::email_ok(&mail) {
            errors.push("电子邮箱格式错误".to_string());
        }
    }
    if !screen.is_empty() {
        if state.db.screen_taken(except, &screen).await.unwrap_or(false) {
            errors.push("昵称已经存在".to_string());
        }
        if !auth::xss_ok(&screen) {
            errors.push("请不要在昵称中使用特殊字符".to_string());
        }
    }
    if !password.is_empty() && password.chars().count() < 6 {
        errors.push("为了保证账户安全, 请输入至少六位的密码".to_string());
    }
    if !updating && password.is_empty() {
        errors.push("必须填写密码".to_string());
    }
    if password.is_empty() {
        if !confirm.is_empty() {
            errors.push("两次输入的密码不一致".to_string());
        }
    } else if password != confirm {
        errors.push("两次输入的密码不一致".to_string());
    }
    if !url.is_empty() && !auth::url_ok(&url) {
        errors.push("个人主页地址格式错误".to_string());
    }
    if updating {
        if uid.is_none() {
            errors.push("用户主键不存在".to_string());
        } else if current.is_none() {
            errors.push("用户不存在".to_string());
        }
    }
    if !errors.is_empty() {
        let mut q = HashMap::new();
        q.insert("error".into(), errors.join("\n"));
        return user_form(state, headers, uid, updating, &q, &form).await;
    }
    let screen = if screen.is_empty() { name.clone() } else { screen };
    let group = form.get("group").cloned().unwrap_or_else(|| "subscriber".into());
    let saved = state
        .db
        .save_user(&UserInput {
            uid: if updating { uid } else { None },
            name,
            password,
            mail,
            url,
            screen: screen.clone(),
            group,
            created: chrono::Utc::now().timestamp(),
        })
        .await;
    match saved {
        Ok(_) => {
            let verb = if updating { "已经被更新" } else { "已经被增加" };
            notice_to("/admin/manage-users.php", &format!("用户 {screen} {verb}"))
        }
        Err(_) => {
            let mut q = HashMap::new();
            q.insert("error".into(), "保存失败".into());
            user_form(state, headers, uid, updating, &q, &form).await
        }
    }
}

async fn users_batch(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, body: String) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let mut deleted = 0;
    if q.get("do").map(|s| s.as_str()) == Some("delete") {
        let rows = state.db.list_users().await.unwrap_or_default();
        for uid in repeated(&body).get("uid[]").cloned().unwrap_or_default() {
            let Ok(uid) = uid.parse::<i64>() else { continue };
            if user_delete_blocked(&rows, user.uid, uid) {
                continue;
            }
            if state.db.delete_user(uid).await.is_ok() {
                deleted += 1;
            }
        }
    }
    if deleted > 0 {
        notice_to("/admin/manage-users.php", "用户已经删除")
    } else {
        notice_to("/admin/manage-users.php", "没有用户被删除")
    }
}

fn user_delete_blocked(rows: &[SessionUser], actor: i64, uid: i64) -> bool {
    if uid == actor {
        return true;
    }
    let Some(target) = rows.iter().find(|r| r.uid == uid) else {
        return true;
    };
    let admins = rows.iter().filter(|r| r.group == "administrator").count();
    target.group == "administrator" && admins <= 1
}

async fn user_delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(uid): Path<i64>) -> Response {
    let Ok((_, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let rows = state.db.list_users().await.unwrap_or_default();
    if user_delete_blocked(&rows, user.uid, uid) || state.db.delete_user(uid).await.is_err() {
        return notice_to("/admin/manage-users.php", "没有用户被删除");
    }
    notice_to("/admin/manage-users.php", "用户已经删除")
}

async fn settings_general(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let tz = site.int("timezone", 28800);
    let mut zones = String::new();
    for (value, label) in TIMEZONES {
        let sel = if tz == *value { " selected" } else { "" };
        zones.push_str(&format!("<option value=\"{value}\"{sel}>{label}</option>"));
    }
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-8 col-tb-offset-2">
<form method="post" action="/action/options-general"><ul class="typecho-option">
<li><label class="typecho-label" for="title">站点名称</label><input id="title" name="title" type="text" class="w-100 text" value="{title}"><p class="description">站点的名称将显示在网页的标题处.</p></li>
<li><label class="typecho-label" for="siteUrl">站点地址</label><input id="siteUrl" name="siteUrl" type="text" class="w-100 mono" value="{site}"><p class="description">站点地址主要用于生成内容的永久链接.</p></li>
<li><label class="typecho-label" for="description">站点描述</label><input id="description" name="description" type="text" class="w-100 text" value="{desc}"><p class="description">站点描述将显示在网页代码的头部.</p></li>
<li><label class="typecho-label" for="keywords">关键词</label><input id="keywords" name="keywords" type="text" class="w-100 text" value="{kw}"><p class="description">请以半角逗号 "," 分割多个关键字.</p></li>
<li><label class="typecho-label">是否允许注册</label>{reg}<p class="description">允许访问者注册到你的网站, 默认的注册用户不享有任何写入权限.</p></li>
<li><label class="typecho-label">XMLRPC 接口</label>{xml}</li>
<li><label class="typecho-label" for="timezone">时区</label><select id="timezone" name="timezone">{zones}</select></li>
<li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li>
</ul></form></div></div>"#,
        title = html_escape(site.get("title")),
        site = html_escape(site.get("siteUrl")),
        desc = html_escape(site.get("description")),
        kw = html_escape(site.get("keywords")),
        reg = radios("allowRegister", &[("0", "不允许"), ("1", "允许")], site.get_or("allowRegister", "0")),
        xml = radios("allowXmlRpc", &[("0", "关闭"), ("1", "仅关闭 Pingback 接口"), ("2", "打开")], site.get_or("allowXmlRpc", "2")),
    );
    layout(&site, &user, "general", "基本设置", None, "container", &main)
}

async fn settings_discussion(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let days = site.int("commentsPostTimeout", 0) / 86400;
    let minutes = site.int("commentsPostInterval", 60) as f64 / 60.0;
    let show = checkbox_group(
        "commentsShow",
        &[
            ("commentsShowCommentOnly", "仅显示评论, 不显示 Pingback 和 Trackback"),
            ("commentsMarkdown", "在评论中使用 Markdown 语法"),
            ("commentsShowUrl", "评论者名称显示时自动加上其个人主页链接"),
            ("commentsUrlNofollow", "对评论者个人主页链接使用 nofollow 属性"),
            ("commentsAvatar", "启用 Gravatar 头像服务"),
            ("commentsPageBreak", "启用分页"),
            ("commentsThreaded", "启用评论回复"),
        ],
        &site,
    );
    let post = checkbox_group(
        "commentsPost",
        &[
            ("commentsRequireModeration", "所有评论必须经过审核"),
            ("commentsWhitelist", "评论者之前须有评论通过了审核"),
            ("commentsRequireMail", "必须填写邮箱"),
            ("commentsRequireURL", "必须填写网址"),
            ("commentsCheckReferer", "检查评论来源页 URL 是否与文章链接一致"),
            ("commentsAntiSpam", "开启反垃圾保护"),
            ("commentsAutoClose", "在文章发布若干天以后自动关闭评论"),
            ("commentsPostIntervalEnable", "同一 IP 发布评论的时间间隔限制"),
        ],
        &site,
    );
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-8 col-tb-offset-2">
<form method="post" action="/action/options-discussion"><ul class="typecho-option">
<li><label class="typecho-label">评论日期格式</label><input class="w-40 mono" name="commentDateFormat" value="{fmt}"><p class="description">具体写法请参考 PHP 日期格式写法.</p></li>
<li><label class="typecho-label">评论列表数目</label><input class="w-20" name="commentsListSize" value="{list}"><p class="description">此数目用于指定显示在侧边栏中的评论列表数目.</p></li>
<li><label class="typecho-label">评论显示</label>{show}
<p class="description">每页评论数 <input class="text num text-s" name="commentsPageSize" value="{page}"> ，默认显示
<select name="commentsPageDisplay"><option value="first"{first}>第一页</option><option value="last"{last}>最后一页</option></select>
，回复层数 <input class="text num text-s" name="commentsMaxNestingLevels" value="{nest}">
，排序 <select name="commentsOrder"><option value="DESC"{desc}>较新的</option><option value="ASC"{asc}>较旧的</option></select></p></li>
<li><label class="typecho-label">评论提交</label>{post}
<p class="description">自动关闭天数 <input class="text num text-s" name="commentsPostTimeout" value="{days}"> ，间隔分钟 <input class="text num text-s" name="commentsPostInterval" value="{minutes}"></p></li>
<li><label class="typecho-label">允许使用的HTML标签和属性</label><textarea class="mono" name="commentsHTMLTagAllowed">{html}</textarea></li>
<li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li>
</ul></form></div></div>"#,
        fmt = html_escape(site.get("commentDateFormat")),
        list = html_escape(site.get_or("commentsListSize", "10")),
        page = site.int("commentsPageSize", 20),
        nest = html_escape(site.get_or("commentsMaxNestingLevels", "5")),
        first = if site.get("commentsPageDisplay") != "last" { " selected" } else { "" },
        last = if site.get("commentsPageDisplay") == "last" { " selected" } else { "" },
        desc = if !site.get("commentsOrder").eq_ignore_ascii_case("ASC") { " selected" } else { "" },
        asc = if site.get("commentsOrder").eq_ignore_ascii_case("ASC") { " selected" } else { "" },
        days = days,
        minutes = minutes,
        html = html_escape(site.get("commentsHTMLTagAllowed")),
    );
    layout(&site, &user, "discussion", "评论设置", None, "container", &main)
}

async fn settings_reading(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let front = site.get("frontPage");
    let recent = if !front.starts_with("page:") { " checked" } else { "" };
    let page_on = if front.starts_with("page:") { " checked" } else { "" };
    let selected_page = front.strip_prefix("page:").unwrap_or("");
    let mut pages = String::new();
    for p in &site.pages {
        let sel = if p.cid.to_string() == selected_page { " selected" } else { "" };
        pages.push_str(&format!("<option value=\"{}\"{sel}>{}</option>", p.cid, html_escape(&p.title)));
    }
    let archive = if site.flag("frontArchive", false) { " checked" } else { "" };
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-8 col-tb-offset-2">
<form method="post" action="/action/options-reading"><ul class="typecho-option">
<li><label class="typecho-label">文章日期格式</label><input class="w-40 mono" name="postDateFormat" value="{fmt}"><p class="description">此格式用于指定显示在文章归档中的日期默认显示格式.</p></li>
<li><label class="typecho-label">站点首页</label>
<span class="multiline"><input type="radio" name="frontPage" value="recent"{recent}> 显示最新发布的文章</span>
<span class="multiline"><input type="radio" name="frontPage" value="page"{page_on}> 使用 <select name="frontPagePage">{pages}</select> 页面作为首页</span>
<span class="multiline"><input type="checkbox" name="frontArchive" value="1"{archive}> 同时将文章列表页路径更改为 /blog/</span>
</li>
<li><label class="typecho-label">文章列表数目</label><input class="w-20" name="postsListSize" value="{side}"><p class="description">此数目用于指定显示在侧边栏中的文章列表数目.</p></li>
<li><label class="typecho-label">每页文章数目</label><input class="w-20" name="pageSize" value="{page}"><p class="description">此数目用于指定文章归档输出时每页显示的文章数目.</p></li>
<li><label class="typecho-label">聚合全文输出</label>{feed}<p class="description">如果你不希望在聚合中输出文章全文, 请使用仅输出摘要选项. 摘要的文字取决于你在文章中使用分隔符的位置.</p></li>
<li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li>
</ul></form></div></div>"#,
        fmt = html_escape(site.get_or("postDateFormat", "Y-m-d")),
        side = html_escape(site.get_or("postsListSize", "10")),
        page = site.page_size(),
        feed = radios("feedFullText", &[("0", "仅输出摘要"), ("1", "全文输出")], site.get_or("feedFullText", "0")),
    );
    layout(&site, &user, "reading", "阅读设置", None, "container", &main)
}

async fn settings_permalink(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Ok((site, user, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let post = route_source(&site, "post");
    let page = decode_rule(&route_source(&site, "page"));
    let category = decode_rule(&route_source(&site, "category"));
    let patterns = [
        ("/archives/[cid:digital]/", "默认风格", "/archives/{cid}/"),
        ("/archives/[slug].html", "wordpress风格", "/archives/{slug}.html"),
        ("/[year:digital:4]/[month:digital:2]/[day:digital:2]/[slug].html", "按日期归档", "/{year}/{month}/{day}/{slug}.html"),
        ("/[category]/[slug].html", "按分类归档", "/{category}/{slug}.html"),
    ];
    let custom = !patterns.iter().any(|(p, _, _)| *p == post);
    let mut radios_html = String::new();
    for (value, label, code) in patterns {
        let sel = if post == value { " checked" } else { "" };
        radios_html.push_str(&format!(
            "<span class=\"multiline\"><input type=\"radio\" name=\"postPattern\" value=\"{value}\"{sel}> {label} <code>{code}</code></span>"
        ));
    }
    let custom_checked = if custom { " checked" } else { "" };
    let custom_value = if custom { decode_rule(&post) } else { String::new() };
    radios_html.push_str(&format!(
        "<span class=\"multiline\"><input type=\"radio\" name=\"postPattern\" value=\"custom\"{custom_checked}> 个性化定义 <input type=\"text\" class=\"w-50 text-s mono\" name=\"customPattern\" value=\"{}\"></span>",
        html_escape(&custom_value)
    ));
    let rewrite = radios("rewrite", &[("0", "不启用"), ("1", "启用")], site.get_or("rewrite", "0"));
    let rewrite_hint = if q.contains_key("rewrite_check") {
        r#"<br><input type="checkbox" name="enableRewriteAnyway" id="enableRewriteAnyway" value="1"> <label for="enableRewriteAnyway">如果你仍然想启用此功能, 请勾选这里</label>"#
    } else {
        ""
    };
    let main = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-8 col-tb-offset-2">
{flash}
<form method="post" action="/action/options-permalink"><ul class="typecho-option">
<li><label class="typecho-label">是否使用地址重写功能</label>{rewrite}<p class="description">地址重写即 rewrite 功能是某些服务器软件提供的优化内部连接的功能.<br>打开此功能可以让你的链接看上去完全是静态地址.{rewrite_hint}</p></li>
<li><label class="typecho-label">自定义文章路径</label>{radios_html}<p class="description">可用参数: <code>{{cid}}</code> <code>{{slug}}</code> <code>{{category}}</code> <code>{{directory}}</code> <code>{{year}}</code> <code>{{month}}</code> <code>{{day}}</code></p></li>
<li><label class="typecho-label">独立页面路径</label><input class="mono w-60" name="pagePattern" value="{page}"><p class="description">可用参数: <code>{{cid}}</code> <code>{{slug}}</code></p></li>
<li><label class="typecho-label">分类路径</label><input class="mono w-60" name="categoryPattern" value="{category}"><p class="description">可用参数: <code>{{mid}}</code> <code>{{slug}}</code></p></li>
<li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li>
</ul></form></div></div>"#,
        flash = flash_html(&q),
        page = html_escape(&page),
        category = html_escape(&category),
    );
    layout(&site, &user, "permalink", "永久链接设置", None, "container", &main)
}

async fn settings_save_general(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    for key in ["title", "siteUrl", "description", "keywords", "allowRegister", "allowXmlRpc", "timezone"] {
        if let Some(value) = form.get(key) {
            let _ = state.db.set_option(key, value).await;
        }
    }
    redirect("/admin/options-general.php")
}

async fn settings_save_discussion(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    let many = repeated(&body);
    for key in [
        "commentDateFormat",
        "commentsListSize",
        "commentsPageSize",
        "commentsPageDisplay",
        "commentsMaxNestingLevels",
        "commentsOrder",
        "commentsHTMLTagAllowed",
    ] {
        if let Some(value) = form.get(key) {
            let _ = state.db.set_option(key, value).await;
        }
    }
    if let Some(days) = form.get("commentsPostTimeout").and_then(|s| s.parse::<i64>().ok()) {
        let _ = state.db.set_option("commentsPostTimeout", &(days.saturating_mul(86400)).to_string()).await;
    }
    if let Some(minutes) = form.get("commentsPostInterval").and_then(|s| s.parse::<f64>().ok()) {
        let secs = (minutes * 60.0).round() as i64;
        let _ = state.db.set_option("commentsPostInterval", &secs.to_string()).await;
    }
    let flags = [
        "commentsShowCommentOnly",
        "commentsMarkdown",
        "commentsShowUrl",
        "commentsUrlNofollow",
        "commentsAvatar",
        "commentsPageBreak",
        "commentsThreaded",
        "commentsRequireModeration",
        "commentsWhitelist",
        "commentsRequireMail",
        "commentsRequireURL",
        "commentsCheckReferer",
        "commentsAntiSpam",
        "commentsAutoClose",
        "commentsPostIntervalEnable",
    ];
    let chosen = many.get("commentsShow").cloned().unwrap_or_default();
    let chosen_post = many.get("commentsPost").cloned().unwrap_or_default();
    for key in flags {
        let on = chosen.iter().any(|v| v == key) || chosen_post.iter().any(|v| v == key);
        let _ = state.db.set_option(key, if on { "1" } else { "0" }).await;
    }
    redirect("/admin/options-discussion.php")
}

async fn settings_save_reading(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let Ok((_, _, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    for key in ["postDateFormat", "postsListSize", "pageSize", "feedFullText"] {
        if let Some(value) = form.get(key) {
            let _ = state.db.set_option(key, value).await;
        }
    }
    let front = if form.get("frontPage").map(|s| s.as_str()) == Some("page") {
        format!("page:{}", form.get("frontPagePage").map(|s| s.as_str()).unwrap_or("0"))
    } else {
        "recent".into()
    };
    let _ = state.db.set_option("frontPage", &front).await;
    let _ = state.db.set_option("frontArchive", if form.contains_key("frontArchive") { "1" } else { "0" }).await;
    redirect("/admin/options-reading.php")
}

async fn settings_save_permalink(State(state): State<Arc<AppState>>, headers: HeaderMap, body: String) -> Response {
    let Ok((site, _, _)) = need(&state, &headers, "administrator").await else {
        return redirect_login();
    };
    let form = pairs(&body);
    let page_raw = form.get("pagePattern").map(|s| s.as_str()).unwrap_or("");
    if !page_raw.contains("{slug}") && !page_raw.contains("{cid}") {
        return error_to("/admin/options-permalink.php", "独立页面路径中没有包含 {cid} 或者 {slug} ");
    }
    let category_raw = form.get("categoryPattern").map(|s| s.as_str()).unwrap_or("");
    if !category_raw.contains("{slug}") && !category_raw.contains("{mid}") && !category_raw.contains("{directory}") {
        return error_to("/admin/options-permalink.php", "分类路径中没有包含 {mid} 或者 {slug} ");
    }
    let enabling = form.get("rewrite").map(|s| s.as_str()) == Some("1") && !site.flag("rewrite", false);
    if enabling && !form.contains_key("enableRewriteAnyway") && !rewrite_probe(&site).await {
        return redirect("/admin/options-permalink.php?rewrite_check=1&error=%E9%87%8D%E5%86%99%E5%8A%9F%E8%83%BD%E6%A3%80%E6%B5%8B%E5%A4%B1%E8%B4%A5%2C%20%E8%AF%B7%E6%A3%80%E6%9F%A5%E4%BD%A0%E7%9A%84%E6%9C%8D%E5%8A%A1%E5%99%A8%E8%AE%BE%E7%BD%AE");
    }
    if let Some(rewrite) = form.get("rewrite") {
        let _ = state.db.set_option("rewrite", rewrite).await;
    }
    let custom = form.get("postPattern").map(|s| s.as_str()) == Some("custom");
    let post = if custom {
        encode_rule(form.get("customPattern").map(|s| s.as_str()).unwrap_or(""))
    } else {
        form.get("postPattern").cloned().unwrap_or_else(|| route_source(&site, "post"))
    };
    let page = encode_rule(page_raw);
    let category = encode_rule(category_raw);
    let pattern_ok = !custom || !rule_conflicts(&site, &post);
    if let Some(blob) = patch_routes(&site, &post, &page, &category) {
        let _ = state.db.set_option("routingTable", &blob).await;
    }
    let notice = if pattern_ok {
        "设置已经保存"
    } else {
        "自定义链接与现有规则存在冲突! 它可能影响解析效率, 建议你重新分配一个规则."
    };
    notice_to("/admin/options-permalink.php", notice)
}

/// 开启伪静态前，向 `siteUrl/action/ajax` 发 `do=remoteCallback`，和 `Permalink::checkRewrite` 一样。
async fn rewrite_probe(site: &Site) -> bool {
    let base = site.get("siteUrl").trim().trim_end_matches('/').to_string();
    if !base.starts_with("http://") {
        return false;
    }
    let url = links::common_url("/action/ajax", &base);
    let generator = site.get("generator").to_string();
    tokio::time::timeout(std::time::Duration::from_millis(1500), probe_http(&url, &generator))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}

async fn probe_http(url: &str, generator: &str) -> anyhow::Result<bool> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    if hostport.is_empty() {
        return Ok(false);
    }
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(80)),
        None => (hostport, 80),
    };
    let generator = generator.replace(['\r', '\n'], "");
    let addr = tokio::net::lookup_host((host, port)).await?.next().ok_or_else(|| anyhow::anyhow!("no address"))?;
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_millis(800),
        tokio::net::TcpStream::connect(addr),
    )
    .await??;
    let body = "do=remoteCallback";
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: {generator}\r\nX-Requested-With: XMLHttpRequest\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(req.as_bytes()).await?;
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(std::time::Duration::from_millis(800), stream.read(&mut buf)).await??;
    let text = String::from_utf8_lossy(&buf[..n]);
    let status_ok = text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200");
    let body_ok = text.split("\r\n\r\n").nth(1).unwrap_or("").starts_with("OK");
    Ok(status_ok && body_ok)
}

fn rule_conflicts(site: &Site, custom: &str) -> bool {
    if custom.is_empty() {
        return false;
    }
    let table = crate::router::RouterTable::compile(&[("custom", custom)]);
    let Some(route) = table.routes.first() else {
        return false;
    };
    site.routes.routes.iter().any(|other| {
        if other.name == "post" || other.name == "page" {
            return false;
        }
        route.regex.is_match(&sample_path(&other.source))
    })
}

fn sample_path(source: &str) -> String {
    decode_rule(source)
        .replace("{cid}", "123")
        .replace("{slug}", "hello")
        .replace("{category}", "default")
        .replace("{year}", "2008")
        .replace("{month}", "08")
        .replace("{day}", "08")
        .replace('{', "")
        .replace('}', "")
}

fn patch_routes(site: &Site, post: &str, page: &str, category: &str) -> Option<String> {
    let raw = site.get("routingTable");
    let mut value = if raw.is_empty() {
        let entries = site
            .routes
            .routes
            .iter()
            .map(|r| {
                (
                    PhpValue::String(r.name.clone()),
                    PhpValue::Array(vec![(PhpValue::String("url".into()), PhpValue::String(r.source.clone()))]),
                )
            })
            .collect();
        PhpValue::Array(entries)
    } else {
        phpval::parse_php(raw).ok()?
    };
    let category_page = format!("{}/[page:digital]/", category.trim_end_matches('/'));
    for (name, url) in [("post", post), ("page", page), ("category", category), ("category_page", category_page.as_str())] {
        if url.is_empty() || url == "/" {
            continue;
        }
        set_route_url(&mut value, name, url);
    }
    Some(phpval::serialize(&value))
}

fn set_route_url(value: &mut PhpValue, name: &str, url: &str) {
    let PhpValue::Array(items) = value else { return };
    for (key, val) in items.iter_mut() {
        if key.as_str() != Some(name) {
            continue;
        }
        if let PhpValue::Array(fields) = val {
            if let Some((_, slot)) = fields.iter_mut().find(|(k, _)| k.as_str() == Some("url")) {
                *slot = PhpValue::String(url.into());
            } else {
                fields.push((PhpValue::String("url".into()), PhpValue::String(url.into())));
            }
            return;
        }
    }
    items.push((
        PhpValue::String(name.into()),
        PhpValue::Array(vec![(PhpValue::String("url".into()), PhpValue::String(url.into()))]),
    ));
}

fn route_source(site: &Site, name: &str) -> String {
    site.routes.routes.iter().find(|r| r.name == name).map(|r| r.source.clone()).unwrap_or_default()
}

fn encode_rule(rule: &str) -> String {
    let rule = rule.trim();
    if rule.is_empty() {
        return String::new();
    }
    let mut s = rule.to_string();
    for (from, to) in [
        ("{cid}", "[cid:digital]"),
        ("{year}", "[year:digital:4]"),
        ("{month}", "[month:digital:2]"),
        ("{day}", "[day:digital:2]"),
        ("{mid}", "[mid:digital]"),
        ("{directory}", "[directory:split:0]"),
        ("{slug}", "[slug]"),
        ("{category}", "[category]"),
    ] {
        s = s.replace(from, to);
    }
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    s
}

fn decode_rule(rule: &str) -> String {
    let mut out = String::new();
    let bytes = rule.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = rule[i + 1..].find(']') {
                let inner = &rule[i + 1..i + 1 + end];
                let name = inner.split(':').next().unwrap_or(inner);
                out.push('{');
                out.push_str(name);
                out.push('}');
                i += end + 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn radios(name: &str, options: &[(&str, &str)], current: &str) -> String {
    let mut html = String::new();
    for (value, label) in options {
        let sel = if *value == current { " checked" } else { "" };
        html.push_str(&format!(
            "<span class=\"multiline\"><input type=\"radio\" name=\"{name}\" value=\"{value}\"{sel}> {label}</span>"
        ));
    }
    html
}

fn checkbox_group(name: &str, options: &[(&str, &str)], site: &Site) -> String {
    let mut html = String::new();
    for (key, label) in options {
        let sel = if site.flag(key, false) { " checked" } else { "" };
        html.push_str(&format!(
            "<span class=\"multiline\"><input type=\"checkbox\" name=\"{name}\" value=\"{key}\"{sel}> {label}</span>"
        ));
    }
    html
}

fn layout(site: &Site, user: &SessionUser, key: &str, title: &str, add: Option<&str>, container: &str, main: &str) -> Response {
    let add_html = add
        .map(|href| format!("<a href=\"{href}\">新增</a>"))
        .unwrap_or_default();
    let html = format!(
        r#"<!DOCTYPE HTML>
<html><head>
<meta charset="UTF-8">
<meta name="renderer" content="webkit">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>{title} - {site_title} - Powered by Typecho</title>
<meta name="robots" content="noindex, nofollow">
<link rel="stylesheet" href="/static/admin/css/normalize.css">
<link rel="stylesheet" href="/static/admin/css/grid.css">
<link rel="stylesheet" href="/static/admin/css/style.css">
<style>.btn-drop.open > .dropdown-menu {{ display: block; }}</style>
</head><body>
{nav}
<div class="main"><div class="{container}">
<div class="typecho-page-title"><h2>{title}{add_html}</h2></div>
{main}
</div></div>
{foot}
{globals}
<script>{js}</script>
</body></html>"#,
        site_title = html_escape(site.get_or("title", "Tiebidli")),
        nav = menu(user, key, site),
        foot = footer(),
        globals = signed_globals(site, user),
        js = ADMIN_JS,
    );
    html_ok(stamp_with(site, Some(user), &html))
}

fn guest(site: &Site, title: &str, class: &str, body: &str) -> Response {
    let html = format!(
        r#"<!DOCTYPE HTML>
<html><head>
<meta charset="UTF-8">
<meta name="renderer" content="webkit">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>{title} - {site_title} - Powered by Typecho</title>
<meta name="robots" content="noindex, nofollow">
<link rel="stylesheet" href="/static/admin/css/normalize.css">
<link rel="stylesheet" href="/static/admin/css/grid.css">
<link rel="stylesheet" href="/static/admin/css/style.css">
</head><body class="{class}">{body}{foot}</body></html>"#,
        site_title = html_escape(site.get_or("title", "Tiebidli")),
        foot = footer(),
    );
    html_ok(stamp_with(site, None, &html))
}

fn signed_globals(site: &Site, user: &SessionUser) -> String {
    let token = auth::security_token(
        site.get("secret"),
        Some((user.auth_code.as_str(), user.uid)),
        &page_url(),
    );
    let upload = links::with_token(&links::action_url(site, "/action/upload", false), &token);
    let attachment = links::with_token(
        &links::action_url(site, "/action/contents-attachment-edit", false),
        &token,
    );
    format!(r#"<script>window.tiebidliUpload="{upload}";window.tiebidliAttachment="{attachment}";</script>"#)
}

/// 给页面里的 `/action/` 地址补上 `Options->index` 和 `_=`。
/// 登录和永久链接保存固定走 `/index.php`，和 `Security::getTokenUrl` 一致。
fn stamp_with(site: &Site, user: Option<&SessionUser>, html: &str) -> String {
    let token = auth::security_token(
        site.get("secret"),
        user.map(|u| (u.auth_code.as_str(), u.uid)),
        &page_url(),
    );
    let re = regex::Regex::new(r#"(href|action|rel)="([^"]*)""#).expect("attr");
    re.replace_all(html, |caps: &regex::Captures| {
        let attr = &caps[1];
        let url = &caps[2];
        let Some(idx) = url.find("/action/") else {
            return caps[0].to_string();
        };
        let rest = &url[idx..];
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let query = query
            .split('&')
            .filter(|part| !part.is_empty() && !part.starts_with("_="))
            .collect::<Vec<_>>()
            .join("&");
        let always = path.contains("/action/login")
            || path.contains("/action/options-permalink")
            || path.contains("/action/backup");
        let signed = links::action_url(site, path, always);
        let with_query = if query.is_empty() {
            signed
        } else {
            format!("{signed}?{query}")
        };
        format!("{attr}=\"{}\"", links::with_token(&with_query, &token))
    })
    .into_owned()
}

fn footer() -> &'static str {
    r#"<div class="typecho-foot" role="contentinfo"><div class="copyright">
<a href="https://typecho.org" class="i-logo-s">Typecho</a>
<p>由 <a href="https://typecho.org">Typecho</a> 强力驱动, 版本 1.2.1</p>
</div><nav class="resource">
<a href="https://docs.typecho.org">帮助文档</a> &bull;
<a href="https://forum.typecho.org">支持论坛</a> &bull;
<a href="https://github.com/typecho/typecho/issues">报告错误</a> &bull;
<a href="https://typecho.org/download">资源下载</a>
</nav></div>"#
}

fn menu(user: &SessionUser, key: &str, site: &Site) -> String {
    let tz = site.timezone();
    let mut groups = Vec::new();
    let mut console = vec![
        ("概要", "/admin/index.php", "dashboard"),
        ("个人设置", "/admin/profile.php", "profile"),
    ];
    if user.pass("administrator") {
        console.push(("插件", "/admin/plugins.php", "plugins"));
        console.push(("外观", "/admin/themes.php", "themes"));
        console.push(("备份", "/admin/backup.php", "backup"));
    }
    groups.push(("控制台", "/admin/index.php", console));
    let mut write = Vec::new();
    if user.pass("contributor") {
        write.push(("撰写文章", "/admin/write-post.php", "write-post"));
    }
    if user.pass("editor") {
        write.push(("创建页面", "/admin/write-page.php", "write-page"));
    }
    if !write.is_empty() {
        groups.push(("撰写", write[0].1, write));
    }
    let mut manage = vec![("文章", "/admin/manage-posts.php", "posts")];
    if user.pass("editor") {
        manage.push(("独立页面", "/admin/manage-pages.php", "pages"));
    }
    manage.push(("评论", "/admin/manage-comments.php", "comments"));
    if user.pass("editor") {
        manage.push(("分类", "/admin/manage-categories.php", "categories"));
        manage.push(("标签", "/admin/manage-tags.php", "tags"));
        manage.push(("文件", "/admin/manage-medias.php", "media"));
    }
    if user.pass("administrator") {
        manage.push(("用户", "/admin/manage-users.php", "users"));
    }
    groups.push(("管理", "/admin/manage-posts.php", manage));
    if user.pass("administrator") {
        groups.push((
            "设置",
            "/admin/options-general.php",
            vec![
                ("基本", "/admin/options-general.php", "general"),
                ("评论", "/admin/options-discussion.php", "discussion"),
                ("阅读", "/admin/options-reading.php", "reading"),
                ("永久链接", "/admin/options-permalink.php", "permalink"),
            ],
        ));
    }
    let mut html = String::from(
        r#"<div class="typecho-head-nav clearfix" role="navigation"><button class="menu-bar" type="button">菜单</button><nav id="typecho-nav-list">"#,
    );
    for (label, href, items) in &groups {
        let focus = if items.iter().any(|(_, _, k)| *k == key) { " focus" } else { "" };
        html.push_str(&format!(
            "<ul class=\"root{focus}\"><li class=\"parent\"><a href=\"{href}\">{label}</a></li><ul class=\"child\">"
        ));
        for (i, (name, url, k)) in items.iter().enumerate() {
            let mut class = String::new();
            if *k == key {
                class.push_str("focus");
            }
            if i + 1 == items.len() {
                if !class.is_empty() {
                    class.push(' ');
                }
                class.push_str("last");
            }
            let class = if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") };
            html.push_str(&format!("<li{class}><a href=\"{url}\">{name}</a></li>"));
        }
        html.push_str("</ul></ul>");
    }
    html.push_str("</nav><div class=\"operate\">");
    html.push_str(&plugins::admin_nav(site));
    let login_title = if user.logged > 0 {
        format!(" title=\"最后登录: {}\"", html_escape(&date_word(user.logged, tz)))
    } else {
        String::new()
    };
    html.push_str(&format!(
        "<a class=\"author\" href=\"/admin/profile.php\"{login_title}>{}</a>",
        html_escape(&user.label())
    ));
    html.push_str("<a class=\"exit\" href=\"/action/logout\">登出</a><a href=\"/\">网站</a></div></div>");
    html
}

fn pager_html(page: usize, pages: usize, base: &str, q: &HashMap<String, String>) -> String {
    if pages <= 1 {
        return String::new();
    }
    let mut html = String::from("<ul class=\"typecho-pager\">");
    for n in 1..=pages {
        let current = if n == page { " class=\"current\"" } else { "" };
        let mut query = q.clone();
        query.insert("page".into(), n.to_string());
        let qs: String = query.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect::<Vec<_>>().join("&");
        html.push_str(&format!("<li{current}><a href=\"{base}?{qs}\">{n}</a></li>"));
    }
    html.push_str("</ul>");
    html
}

const ADMIN_JS: &str = r##"
document.querySelectorAll('.btn-drop').forEach(function (drop) {
  var btn = drop.querySelector('.dropdown-toggle');
  if (btn) btn.addEventListener('click', function (e) {
    e.preventDefault();
    e.stopPropagation();
    drop.classList.toggle('open');
  });
  drop.querySelectorAll('.dropdown-menu a').forEach(function (a) {
    a.addEventListener('click', function (e) {
      var form = document.querySelector('form.operate-form');
      if (!form) return;
      e.preventDefault();
      e.stopPropagation();
      var lang = a.getAttribute('lang');
      if (lang && !window.confirm(lang)) return;
      form.action = a.getAttribute('href');
      form.submit();
    });
  });
});
document.addEventListener('click', function () {
  document.querySelectorAll('.btn-drop.open').forEach(function (d) { d.classList.remove('open'); });
});
document.querySelectorAll('.typecho-table-select-all').forEach(function (box) {
  box.addEventListener('change', function () {
    document.querySelectorAll('form.operate-form tbody input[type=checkbox]').forEach(function (c) {
      c.checked = box.checked;
      var tr = c.closest('tr');
      if (tr) tr.classList.toggle('checked', c.checked);
    });
  });
});
document.querySelectorAll("a[href^='#']").forEach(function (a) {
  a.addEventListener('click', function (e) {
    var id = a.getAttribute('href').slice(1);
    var panel = document.getElementById(id);
    if (!panel || !panel.classList.contains('tab-content')) return;
    e.preventDefault();
    var ul = a.closest('ul');
    if (ul) ul.querySelectorAll('li').forEach(function (li) { li.classList.remove('active'); });
    if (a.parentElement) a.parentElement.classList.add('active');
    document.querySelectorAll('.tab-content').forEach(function (p) { p.classList.add('hidden'); });
    panel.classList.remove('hidden');
  });
});
var advance = document.getElementById('advance-panel-btn');
if (advance) advance.addEventListener('click', function () {
  var p = document.getElementById('advance-panel');
  if (!p) return;
  p.style.display = p.style.display === 'block' ? 'none' : 'block';
});
var vis = document.getElementById('visibility');
if (vis) vis.addEventListener('change', function () {
  var pwd = document.getElementById('post-password');
  if (pwd) pwd.classList.toggle('hidden', vis.value !== 'password');
});
var bar = document.querySelector('button.menu-bar');
if (bar) bar.addEventListener('click', function () { bar.classList.toggle('focus'); });
document.querySelectorAll('a[lang]').forEach(function (a) {
  if (a.closest('.dropdown-menu')) return;
  a.addEventListener('click', function (e) {
    if (!window.confirm(a.getAttribute('lang'))) e.preventDefault();
  });
});
var btnPreview = document.getElementById('btn-preview');
var btnCancel = document.getElementById('btn-cancel-preview');
if (btnPreview) btnPreview.addEventListener('click', function () {
  var form = document.querySelector('form[name=write_post],form[name=write_page]');
  if (!form) return;
  var frame = document.querySelector('iframe.preview-frame');
  if (!frame) {
    frame = document.createElement('iframe');
    frame.className = 'preview-frame';
    frame.setAttribute('name', 'typecho-preview');
    document.body.appendChild(frame);
  }
  frame.style.height = Math.max(window.innerHeight - 53, 480) + 'px';
  var oldAction = form.getAttribute('action');
  var oldTarget = form.getAttribute('target');
  form.setAttribute('action', '/admin/preview');
  form.setAttribute('target', 'typecho-preview');
  form.submit();
  form.setAttribute('action', oldAction || '');
  if (oldTarget) form.setAttribute('target', oldTarget);
  else form.removeAttribute('target');
  document.body.classList.add('fullscreen');
  document.body.classList.add('preview');
});
if (btnCancel) btnCancel.addEventListener('click', function () {
  document.body.classList.remove('preview');
  document.body.classList.remove('fullscreen');
  document.querySelectorAll('iframe.preview-frame').forEach(function (frame) { frame.remove(); });
});
function fieldDelete(button) {
  button.addEventListener('click', function () {
    if (!window.confirm('确认要删除此字段吗?')) return;
    var tr = button.closest('tr');
    if (tr) tr.remove();
  });
}
document.querySelectorAll('#custom-field tbody button.btn-xs').forEach(fieldDelete);
var addField = document.querySelector('#custom-field button.operate-add');
if (addField) addField.addEventListener('click', function () {
  var body = document.querySelector('#custom-field tbody');
  if (!body) return;
  var tr = document.createElement('tr');
  tr.innerHTML = '<td><input type="text" name="fieldNames[]" placeholder="字段名称" class="text-s w-100"></td>'
    + '<td><select name="fieldTypes[]"><option value="str">字符</option><option value="int">整数</option><option value="float">小数</option></select></td>'
    + '<td><textarea name="fieldValues[]" placeholder="字段值" class="text-s w-100" rows="2"></textarea></td>'
    + '<td><button type="button" class="btn btn-xs">删除</button></td>';
  body.appendChild(tr);
  var btn = tr.querySelector('button');
  if (btn) fieldDelete(btn);
});
var fieldExpand = document.getElementById('custom-field-expand');
if (fieldExpand) fieldExpand.addEventListener('click', function (e) {
  e.preventDefault();
  var icon = fieldExpand.querySelector('i');
  var section = fieldExpand.parentElement;
  if (icon) {
    if (icon.classList.contains('i-caret-right')) {
      icon.classList.remove('i-caret-right');
      icon.classList.add('i-caret-down');
    } else {
      icon.classList.remove('i-caret-down');
      icon.classList.add('i-caret-right');
    }
  }
  if (section) section.classList.toggle('fold');
});
document.querySelectorAll('.operate-edit').forEach(function (link) {
  link.addEventListener('click', function (e) {
    e.preventDefault();
    var tr = link.closest('tr');
    if (!tr) return;
    var raw = tr.getAttribute('data-comment') || '{}';
    var comment = {};
    try { comment = JSON.parse(raw); } catch (err) { comment = {}; }
    tr.style.display = 'none';
    var id = tr.id;
    var edit = document.createElement('tr');
    edit.className = 'comment-edit';
    edit.innerHTML = '<td> </td><td colspan="2" valign="top"><form method="post" action="' + link.getAttribute('rel') + '" class="comment-edit-info">'
      + '<p><label for="' + id + '-author">用户名</label><input class="text-s w-100" id="' + id + '-author" name="author" type="text"></p>'
      + '<p><label for="' + id + '-mail">电子邮箱</label><input class="text-s w-100" type="email" name="mail" id="' + id + '-mail"></p>'
      + '<p><label for="' + id + '-url">个人主页</label><input class="text-s w-100" type="text" name="url" id="' + id + '-url"></p></form></td>'
      + '<td valign="top"><form method="post" action="' + link.getAttribute('rel') + '" class="comment-edit-content"><p><label for="' + id + '-text">内容</label>'
      + '<textarea name="text" id="' + id + '-text" rows="6" class="w-90 mono"></textarea></p>'
      + '<p><button type="submit" class="btn btn-s primary">提交</button> <button type="button" class="btn btn-s cancel">取消</button></p></form></td>';
    tr.parentNode.insertBefore(edit, tr.nextSibling);
    edit.querySelector('input[name=author]').value = comment.author || '';
    edit.querySelector('input[name=mail]').value = comment.mail || '';
    edit.querySelector('input[name=url]').value = comment.url || '';
    edit.querySelector('textarea[name=text]').value = comment.text || '';
    edit.querySelector('textarea[name=text]').focus();
    edit.querySelector('.cancel').addEventListener('click', function () {
      tr.style.display = '';
      edit.remove();
    });
    edit.querySelectorAll('form').forEach(function (form) {
      form.addEventListener('submit', function (ev) {
        ev.preventDefault();
        edit.querySelectorAll('form').forEach(function (one) {
          new FormData(one).forEach(function (value, key) { comment[key] = value; });
        });
        var meta = '<strong class="comment-author">' + (comment.url ? '<a target="_blank" href="' + comment.url + '">' + comment.author + '</a>' : comment.author) + '</strong>';
        if (comment.mail) meta += '<br><span><a href="mailto:' + comment.mail + '">' + comment.mail + '</a></span>';
        var metaNode = tr.querySelector('.comment-meta');
        var contentNode = tr.querySelector('.comment-content');
        if (metaNode) metaNode.textContent = '';
        if (metaNode) metaNode.innerHTML = meta;
        if (contentNode) contentNode.innerHTML = '<p>' + comment.text + '</p>';
        tr.setAttribute('data-comment', JSON.stringify(comment));
        var body = new URLSearchParams();
        Object.keys(comment).forEach(function (key) { body.append(key, comment[key] == null ? '' : comment[key]); });
        fetch(form.getAttribute('action'), { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: body.toString() })
          .then(function (res) { return res.json(); })
          .then(function (data) {
            if (data && data.comment && contentNode) contentNode.innerHTML = data.comment.content;
          });
        tr.style.display = '';
        edit.remove();
      });
    });
  });
});
document.querySelectorAll('.dropdown-menu button.merge').forEach(function (btn) {
  btn.addEventListener('click', function (e) {
    e.preventDefault();
    e.stopPropagation();
    var form = btn.closest('form');
    if (!form) return;
    form.setAttribute('action', btn.getAttribute('rel') || '');
    form.submit();
  });
});
document.querySelectorAll('button.btn-operate').forEach(function (btn) {
  btn.addEventListener('click', function (e) {
    e.preventDefault();
    var lang = btn.getAttribute('lang');
    if (lang && !window.confirm(lang)) return;
    var form = document.querySelector('form.operate-form');
    if (!form) return;
    form.setAttribute('action', btn.getAttribute('href') || '');
    form.submit();
  });
});
document.querySelectorAll('button.operate-delete').forEach(function (btn) {
  btn.addEventListener('click', function (e) {
    var lang = btn.getAttribute('lang');
    if (lang && !window.confirm(lang)) e.preventDefault();
  });
});
var attachmentUrl = document.getElementById('attachment-url');
if (attachmentUrl) attachmentUrl.addEventListener('click', function () { attachmentUrl.select(); });
var replaceForm = document.querySelector('#upload-panel form');
if (replaceForm) {
  var replaceInput = replaceForm.querySelector('input[type=file]');
  if (replaceInput) replaceInput.addEventListener('change', function () {
    if (replaceInput.files && replaceInput.files.length) replaceForm.submit();
  });
}
function insertFileToEditor(file, url, isImage) {
  var textarea = document.getElementById('text');
  if (!textarea) return;
  var safeUrl = String(url).replace(/"/g, '&quot;');
  var safeName = String(file).replace(/"/g, '&quot;');
  var html = (isImage === 1 || isImage === '1' || isImage === true)
    ? '<img src="' + safeUrl + '" alt="' + safeName + '" />'
    : '<a href="' + safeUrl + '">' + safeName + '</a>';
  var start = textarea.selectionStart || 0;
  var end = textarea.selectionEnd || 0;
  var val = textarea.value;
  textarea.value = val.slice(0, start) + html + val.slice(end);
  var pos = start + html.length;
  textarea.selectionStart = textarea.selectionEnd = pos;
  textarea.focus();
}
function updateAttachmentNumber() {
  var btn = document.getElementById('tab-files-btn');
  if (!btn) return;
  var count = document.querySelectorAll('#file-list li .insert').length;
  var balloon = btn.querySelector('.balloon');
  if (count > 0) {
    if (!balloon) {
      btn.appendChild(document.createTextNode(' '));
      balloon = document.createElement('span');
      balloon.className = 'balloon';
      btn.appendChild(balloon);
    }
    balloon.textContent = String(count);
  } else if (balloon) {
    balloon.remove();
  }
}
function bindFileItem(li) {
  var insert = li.querySelector('a.insert');
  if (insert) insert.addEventListener('click', function (e) {
    e.preventDefault();
    insertFileToEditor(insert.textContent || '', li.getAttribute('data-url') || '', li.getAttribute('data-image'));
  });
  var del = li.querySelector('a.delete');
  if (del) del.addEventListener('click', function (e) {
    e.preventDefault();
    var name = insert ? (insert.textContent || '') : '';
    if (!window.confirm('确认要删除文件 ' + name + ' 吗?')) return;
    var cid = li.getAttribute('data-cid');
    var body = new URLSearchParams();
    body.set('ajax', '1');
    fetch(window.tiebidliAttachment + (window.tiebidliAttachment.indexOf('?') >= 0 ? '&' : '?') + 'do=delete&cid=' + encodeURIComponent(cid), { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: body.toString() })
      .then(function () { li.remove(); updateAttachmentNumber(); });
  });
}
function uploadFiles(fileList) {
  var list = document.getElementById('file-list');
  var parent = document.querySelector('#side-upload input[name=parent]');
  if (!list || !fileList) return;
  Array.prototype.forEach.call(fileList, function (file) {
    var data = new FormData();
    data.append('file', file);
    data.append('ajax', '1');
    data.append('parent', parent ? parent.value : '0');
    fetch(window.tiebidliUpload, { method: 'POST', body: data })
      .then(function (res) { return res.json(); })
      .then(function (payload) {
        if (!payload || !payload[1] || !list) return;
        var dataFile = payload[1];
        var image = dataFile.isImage ? '1' : '0';
        var li = document.createElement('li');
        li.setAttribute('data-cid', dataFile.cid);
        li.setAttribute('data-url', dataFile.url);
        li.setAttribute('data-image', image);
        li.innerHTML = '<input type="hidden" name="attachment[]" value="' + dataFile.cid + '">'
          + '<a class="insert" title="点击插入文件" href="#">' + dataFile.title + '</a><div class="info">' + dataFile.bytes
          + ' <a class="file" target="_blank" href="/admin/media.php?cid=' + dataFile.cid + '" title="编辑"><i class="i-edit"></i></a>'
          + ' <a href="#" class="delete" title="删除"><i class="i-delete"></i></a></div>';
        list.appendChild(li);
        bindFileItem(li);
        updateAttachmentNumber();
      });
  });
}
document.querySelectorAll('#file-list li').forEach(bindFileItem);
var uploadLink = document.querySelector('#tab-files .upload-file');
var uploadInput = document.getElementById('upload-file');
if (uploadLink && uploadInput) {
  uploadLink.addEventListener('click', function (e) {
    e.preventDefault();
    uploadInput.click();
  });
  uploadInput.addEventListener('change', function () {
    uploadFiles(uploadInput.files);
    uploadInput.value = '';
  });
}
var dropArea = document.querySelector('#tab-files .upload-area');
if (dropArea) {
  ['dragenter', 'dragover'].forEach(function (name) {
    dropArea.addEventListener(name, function (e) {
      e.preventDefault();
      if (dropArea.parentElement) dropArea.parentElement.classList.add('drag');
    });
  });
  ['dragleave', 'drop'].forEach(function (name) {
    dropArea.addEventListener(name, function (e) {
      if (dropArea.parentElement) dropArea.parentElement.classList.remove('drag');
      if (name === 'drop') {
        e.preventDefault();
        if (e.dataTransfer) uploadFiles(e.dataTransfer.files);
      }
    });
  });
}
document.querySelectorAll('.operate-reply').forEach(function (link) {
  link.addEventListener('click', function (e) {
    e.preventDefault();
    var td = link.closest('td');
    if (!td) return;
    var old = td.querySelector('.comment-reply');
    if (old) { old.remove(); return; }
    var form = document.createElement('form');
    form.method = 'post';
    form.action = link.getAttribute('rel');
    form.className = 'comment-reply';
    form.innerHTML = '<p><label for="text" class="sr-only">内容</label><textarea id="text" name="text" class="w-90 mono" rows="3"></textarea></p>'
      + '<p><button type="submit" class="btn btn-s primary">回复</button> <button type="button" class="btn btn-s cancel">取消</button></p>';
    var action = td.querySelector('.comment-action');
    td.insertBefore(form, action);
    form.querySelector('.cancel').addEventListener('click', function () { form.remove(); });
    form.querySelector('textarea').focus();
    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      var tr = form.closest('tr');
      var reply = document.createElement('div');
      reply.className = 'comment-reply-content';
      var text = form.querySelector('textarea').value;
      reply.innerHTML = '<p>' + text + '</p>';
      var content = tr.querySelector('.comment-content');
      if (content) content.parentNode.insertBefore(reply, content.nextSibling);
      var body = new URLSearchParams(new FormData(form));
      fetch(form.action, { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: body.toString() })
        .then(function (res) { return res.json(); })
        .then(function (data) {
          if (data && data.comment) reply.innerHTML = data.comment.content;
        });
      form.remove();
    });
  });
});
"##;

const TIMEZONES: &[(i64, &str)] = &[
    (0, "格林威治(子午线)标准时间 (GMT)"),
    (3600, "中欧标准时间 (GMT +1)"),
    (7200, "东欧标准时间 (GMT +2)"),
    (10800, "莫斯科时间 (GMT +3)"),
    (14400, "第比利斯时间 (GMT +4)"),
    (18000, "新德里时间 (GMT +5)"),
    (21600, "科伦坡时间 (GMT +6)"),
    (25200, "曼谷雅加达 (GMT +7)"),
    (28800, "北京时间 (GMT +8)"),
    (32400, "东京平壤时间 (GMT +9)"),
    (36000, "悉尼关岛时间 (GMT +10)"),
    (39600, "所罗门群岛 (GMT +11)"),
    (43200, "惠灵顿时间 (GMT +12)"),
    (-3600, "佛德尔群岛 (GMT -1)"),
    (-7200, "大西洋中部时间 (GMT -2)"),
    (-10800, "布宜诺斯艾利斯 (GMT -3)"),
    (-14400, "智利巴西 (GMT -4)"),
    (-18000, "纽约渥太华 (GMT -5)"),
    (-21600, "墨西哥城时间 (GMT -6)"),
    (-25200, "美国丹佛时间 (GMT -7)"),
    (-28800, "美国旧金山时间 (GMT -8)"),
    (-32400, "阿拉斯加时间 (GMT -9)"),
    (-36000, "夏威夷群岛 (GMT -10)"),
    (-39600, "东萨摩亚群岛 (GMT -11)"),
    (-43200, "艾尼威托克岛 (GMT -12)"),
];

async fn need(state: &AppState, headers: &HeaderMap, group: &str) -> Result<(Site, SessionUser, String), ()> {
    let site = load_site(state).await.map_err(|_| ())?;
    let root = auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, &site));
    let cookies = cookies_from(headers);
    let user = auth::session_user(&state.db, &root, &cookies).await.map_err(|_| ())?;
    match user {
        Some(user) if user.pass(group) => Ok((site, user, root)),
        _ => Err(()),
    }
}

async fn session(state: &AppState, headers: &HeaderMap) -> Result<Option<SessionUser>, sqlx::Error> {
    let root = root_of(state, headers).await;
    let cookies = cookies_from(headers);
    auth::session_user(&state.db, &root, &cookies).await
}

async fn root_of(state: &AppState, _headers: &HeaderMap) -> String {
    if let Ok(site) = load_site(state).await {
        auth::site_root(&state.cfg.site_url, &links::site_base(&state.cfg.site_url, &site))
    } else {
        auth::site_root(&state.cfg.site_url, "")
    }
}

async fn load_site(state: &AppState) -> Result<Site, sqlx::Error> {
    state.db.load_site(chrono::Utc::now().timestamp()).await
}

fn html_ok(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "页面失败"))
}

fn plain(status: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(msg.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from(msg.to_string())))
}

fn redirect(loc: &str) -> Response {
    redirect_with(loc, &HeaderMap::new())
}

fn redirect_with(loc: &str, extra: &HeaderMap) -> Response {
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

fn pairs(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in split_form(raw) {
        map.insert(k, v);
    }
    map
}

fn repeated(raw: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in split_form(raw) {
        map.entry(k).or_default().push(v);
    }
    map
}

fn split_form(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = decode(k);
        if !k.is_empty() {
            out.push((k, decode(v)));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let spaced = s.replace('+', " ");
    urlencoding::decode(&spaced).map(|c| c.into_owned()).unwrap_or(spaced)
}

fn cookies_from(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for val in headers.get_all(header::COOKIE) {
        let Ok(s) = val.to_str() else { continue };
        for part in s.split(';') {
            let Some((k, v)) = part.trim().split_once('=') else { continue };
            map.insert(k.to_string(), decode(v));
        }
    }
    map
}

fn push_cookie(headers: &mut HeaderMap, name: &str, value: &str, max_age: Option<i64>) {
    let enc = urlencoding::encode(value);
    let mut line = format!("{name}={enc}; Path=/; SameSite=Lax");
    if let Some(age) = max_age {
        line.push_str(&format!("; Max-Age={age}"));
    }
    if let Ok(v) = HeaderValue::from_str(&line) {
        headers.append(header::SET_COOKIE, v);
    }
}

fn parse_local(raw: &str, tz: i64) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(n) = raw.parse::<i64>() {
        if raw.len() >= 9 {
            return Some(n);
        }
    }
    for fmt in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(dt.and_utc().timestamp() - tz);
        }
        if fmt == "%Y-%m-%d" {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, fmt) {
                return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp() - tz);
            }
        }
    }
    None
}

fn date_word(ts: i64, tz: i64) -> String {
    crate::text::format_php_date("Y-m-d H:i", ts, tz)
}

fn date_short(ts: i64, tz: i64) -> String {
    crate::text::format_php_date("n.j", ts, tz)
}

fn gravatar(prefix: &str, mail: &str) -> String {
    if mail.is_empty() {
        return String::new();
    }
    let hash = auth::md5_hex(mail.trim().to_ascii_lowercase().as_bytes());
    format!("<img class=\"avatar\" src=\"{prefix}{hash}?s=40&amp;d=mm\" width=\"40\" height=\"40\" alt=\"\">")
}

fn safe_next(raw: &str, site_base: &str) -> String {
    let raw = raw.trim();
    let blocked = |s: &str| s.contains("/admin/login");
    if raw.is_empty() || blocked(raw) || raw.starts_with("//") {
        return "/admin/index.php".into();
    }
    if raw == "/admin" || raw.starts_with("/admin/") || raw.starts_with("/admin?") {
        return raw.to_string();
    }
    let base = site_base.trim_end_matches('/');
    if !base.is_empty() && (raw == base || raw.starts_with(&format!("{base}/"))) && !blocked(raw) {
        return raw.to_string();
    }
    "/admin/index.php".into()
}
