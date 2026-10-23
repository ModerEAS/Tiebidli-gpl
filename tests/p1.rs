use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tiebidli::db::Db;
use tiebidli::{build_router, AppState, Config};
use tower::ServiceExt;

static N: AtomicU64 = AtomicU64::new(1000);

async fn boot() -> (axum::Router, Db) {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "tiebidli-p1-{n}-{}.db",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let mut cfg = Config::for_test(&url);
    let uploads = path.with_extension("uploads");
    std::fs::create_dir_all(&uploads).expect("uploads");
    cfg.uploads_dir = Some(uploads);
    let themes = path.with_extension("themes");
    let theme_dir = themes.join("default");
    std::fs::create_dir_all(theme_dir.join("assets/css")).expect("theme dir");
    std::fs::write(theme_dir.join("assets/css/style.css"), "/* test */").expect("marker");
    cfg.theme_dir = theme_dir;
    let backups = path.with_extension("backups");
    std::fs::create_dir_all(&backups).expect("backups");
    cfg.backup_dir = backups;
    cfg.template_dir = None;
    let db = Db::connect(&url, &cfg.prefix).await.expect("connect");
    for stmt in include_str!("../schema/typecho-1.2-sqlite.sql").split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt).execute(&db.pool).await.expect("schema");
    }
    seed(&db).await;
    let app = build_router(Arc::new(AppState::new(db.clone(), cfg)));
    (app, db)
}

async fn seed(db: &Db) {
    sqlx::query(
        r#"INSERT INTO "typecho_users"
        ("uid","name","password","mail","url","screenName","created","activated","logged","group","authCode")
        VALUES (1, 'admin', 'x', 'admin@example.com', 'https://example.com', '站长', 1600000000, 1, 0, 'administrator', NULL)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    for (cid, title, slug, kind) in [
        (1_i64, "你好世界", "hello-world", "post"),
        (6_i64, "关于", "about", "page"),
    ] {
        sqlx::query(
            r#"INSERT INTO "typecho_contents"
            ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
            VALUES (?, ?, ?, 1700000000, 1700000100, '正文', 0, 1, NULL, ?, 'publish', '', 0, '1', '1', '1', 0)"#,
        )
        .bind(cid)
        .bind(title)
        .bind(slug)
        .bind(kind)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE uid = 1"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();
    for (name, value) in [
        ("title", "测试博客"),
        ("description", "描述"),
        ("siteUrl", "http://test.local"),
        ("pageSize", "5"),
        ("timezone", "0"),
        ("frontPage", "recent"),
        ("rewrite", "1"),
        ("secret", "test-secret"),
    ] {
        sqlx::query(r#"INSERT INTO "typecho_options" ("name","user","value") VALUES (?, 0, ?)"#)
            .bind(name)
            .bind(value)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String, Vec<u8>, axum::http::HeaderMap) {
    let res = app.clone().oneshot(req).await.expect("oneshot");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.expect("body").to_bytes().to_vec();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, text, bytes, headers)
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, String, Vec<u8>, axum::http::HeaderMap) {
    call(app, Request::builder().uri(path).body(Body::empty()).unwrap()).await
}

async fn post_form(app: &axum::Router, path: &str, form: &str, cookie: &str, referer: &str) -> (StatusCode, String, axum::http::HeaderMap) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", referer);
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let (s, t, _, h) = call(app, b.body(Body::from(form.to_string())).unwrap()).await;
    (s, t, h)
}

async fn auth_code(db: &Db) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(r#"SELECT authCode FROM "typecho_users" WHERE uid = 1"#)
        .fetch_optional(&db.pool)
        .await
        .unwrap()
        .flatten()
        .filter(|s| !s.is_empty())
}

async fn sign_action(db: &Db, path: &str) -> (String, &'static str) {
    let referer = "http://test.local/admin/index.php";
    let code = auth_code(db).await;
    let token = tiebidli::auth::security_token("test-secret", code.as_deref().map(|c| (c, 1)), referer);
    let path = if path.contains('?') { format!("{path}&_={token}") } else { format!("{path}?_={token}") };
    (path, referer)
}

async fn login(app: &axum::Router) -> String {
    let referer = "http://test.local/admin/login.php";
    let token = tiebidli::auth::security_token("test-secret", None, referer);
    let mut b = Request::builder()
        .method("POST")
        .uri(format!("/index.php/action/login?_={token}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", referer);
    let (status, _, _, headers) = call(app, b.body(Body::from("name=admin&password=adminpass")).unwrap()).await;
    assert_eq!(status, StatusCode::FOUND);
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

async fn activate(app: &axum::Router, db: &Db, cookie: &str, name: &str) {
    let (path, referer) = sign_action(db, &format!("/action/plugins-edit?activate={name}")).await;
    let (status, _, headers) = post_form(app, &path, "", cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND, "{name} activate");
    let loc = headers.get("location").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(loc.contains("notice="), "{loc}");
}

#[tokio::test]
async fn sitemap_gated_and_renders_posts_and_pages() {
    let (app, db) = boot().await;
    let (status, _, _, _) = get(&app, "/sitemap.xml").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _, _) = get(&app, "/usr/plugins/Sitemap/sitemap.xsl").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Sitemap").await;

    let (status, body, _, headers) = get(&app, "/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("xml"));
    assert!(body.contains("<urlset"), "{body}");
    assert!(body.contains("/archives/1/"), "{body}");
    assert!(body.contains("/about.html"), "{body}");
    assert!(body.contains("<priority>0.5</priority>"), "{body}");
    assert!(body.contains("<priority>0.8</priority>"), "{body}");
    assert!(body.contains("<changefreq>always</changefreq>"), "{body}");
    assert!(body.contains("sitemap.xsl"), "{body}");
    // 伪静态与非伪静态都要可用。
    let (status, body2, _, _) = get(&app, "/index.php/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK, "{body2}");
    assert!(body2.contains("/archives/1/"), "{body2}");

    let (status, xsl, _, headers) = get(&app, "/usr/plugins/Sitemap/sitemap.xsl").await;
    assert_eq!(status, StatusCode::OK, "{xsl}");
    assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("xsl"));
    assert!(xsl.contains("XML Sitemap"), "{xsl}");
}

#[tokio::test]
async fn access_track_gif_logs_and_views_count_once_per_cookie() {
    let (app, db) = boot().await;
    let (status, _, _, _) = get(&app, "/access/log/track.gif?u=/archives/1/&cid=1&mid=0&rand=1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Access").await;

    // 日志表与 views 列由激活建好。
    assert!(tiebidli::access::table_exists(&db).await);
    let probe: Result<Option<i64>, _> = sqlx::query_scalar(r#"SELECT views FROM "typecho_contents" LIMIT 1"#)
        .fetch_optional(&db.pool)
        .await;
    assert!(probe.is_ok());

    let (status, _, bytes, headers) = get(&app, "/access/log/track.gif?u=/archives/1/&cid=1&mid=0&rand=123").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("gif"));
    assert!(bytes.starts_with(b"GIF89a"), "{bytes:?}");
    let n: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_access_log""#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let cid: Option<i64> = sqlx::query_scalar(r#"SELECT content_id FROM "typecho_access_log" LIMIT 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(cid, Some(1));

    // 后端写入模式默认不注入前端脚本。
    let (status, home, _, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!home.contains("window.Access.track"), "{home}");

    // 切到前端写入后页脚注入打点。
    let (path, referer) = sign_action(&db, "/action/plugins-edit?config=Access").await;
    let (status, _, _) = post_form(&app, &path, "pageSize=10&isDrop=1&writeType=1&canAnalytize=1", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, home, _, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("window.Access.track"), "{home}");
    assert!(home.contains("/access/log/track.gif"), "{home}");

    // 浏览数：首次 +1，带 cookie 重复访问不再加。
    let (status, single, _, headers) = get(&app, "/archives/1/").await;
    assert_eq!(status, StatusCode::OK, "{single}");
    assert!(single.contains("浏览次数"), "{single}");
    assert_eq!(tiebidli::access::get_views(&db, 1).await, 1);
    let set_cookies = headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join("\n");
    assert!(set_cookies.contains("extend_contents_views"), "{set_cookies}");
    let views_cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");
    let (status, _, _, _) = call(
        &app,
        Request::builder().uri("/archives/1/").header("cookie", &views_cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tiebidli::access::get_views(&db, 1).await, 1);

    // 精简控制台可见日志与 PV。
    let (path, referer) = sign_action(&db, "/admin/access-console.php").await;
    let _ = (path, referer);
    let (status, console, _, _) = call(
        &app,
        Request::builder().uri("/admin/access-console.php").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{console}");
    assert!(console.contains("访问日志"), "{console}");
}

#[tokio::test]
async fn links_crud_sort_and_action_compat() {
    let (app, db) = boot().await;
    let cookie = login(&app).await;

    // 未启用时管理页与 action 不可用。
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/admin/links.php").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "{body}");

    activate(&app, &db, &cookie, "Links").await;
    assert!(tiebidli::friend_links::table_exists(&db).await);

    // 管理页可打开。
    let (status, page, _, _) = call(
        &app,
        Request::builder().uri("/admin/links.php").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("友情链接"), "{page}");
    assert!(page.contains("/action/links-edit"), "{page}");

    // insert 两条。
    for (name, url, sort) in [("示例站", "https://example.com", "ten"), ("内页站", "https://inner.example", "one")] {
        let (path, referer) = sign_action(&db, "/action/links-edit?do=insert").await;
        let form = format!("name={}&url={}&sort={}&image=&description=d&do=insert", urlencoding::encode(name), urlencoding::encode(url), sort);
        let (status, _, headers) = post_form(&app, &path, &form, &cookie, referer).await;
        assert_eq!(status, StatusCode::FOUND);
        assert!(headers.get("location").unwrap().to_str().unwrap().contains("notice="));
    }
    let all = tiebidli::friend_links::list(&db).await.unwrap();
    assert_eq!(all.len(), 2);
    let ten = tiebidli::friend_links::list_by_sort(&db, "ten").await.unwrap();
    assert_eq!(ten.len(), 1);
    assert_eq!(ten[0].name, "示例站");

    // 校验失败：空名称。
    let (path, referer) = sign_action(&db, "/action/links-edit?do=insert").await;
    let (status, _, headers) = post_form(&app, &path, "name=&url=https://x.example&sort=ten&do=insert", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(headers.get("location").unwrap().to_str().unwrap().contains("error="));

    // update 第一条。
    let first = all[0].lid;
    let (path, referer) = sign_action(&db, "/action/links-edit?do=update").await;
    let form = format!("lid={first}&name={}&url=https://example.com&sort=good&image=&description=&do=update", urlencoding::encode("示例站改"));
    let (status, _, _) = post_form(&app, &path, &form, &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    let got = tiebidli::friend_links::get(&db, first).await.unwrap().unwrap();
    assert_eq!(got.name, "示例站改");
    assert_eq!(got.sort, "good");

    // sort：把第二条排到前面。
    let second = all[1].lid;
    let (path, referer) = sign_action(&db, "/action/links-edit?do=sort").await;
    let (status, _, _) = post_form(&app, &path, &format!("lid%5B%5D={second}&lid%5B%5D={first}&do=sort"), &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    let ordered = tiebidli::friend_links::list(&db).await.unwrap();
    assert_eq!(ordered[0].lid, second);
    assert_eq!(ordered[1].lid, first);

    // delete。
    let (path, referer) = sign_action(&db, "/action/links-edit?do=delete").await;
    let (status, _, _) = post_form(&app, &path, &format!("lid%5B%5D={first}&do=delete"), &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    let left = tiebidli::friend_links::list(&db).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].lid, second);

    // 伪静态关闭时 action 仍可用（/index.php/action/links-edit）。
    let (path, referer) = sign_action(&db, "/index.php/action/links-edit?do=insert").await;
    let (status, _, _) = post_form(&app, &path, "name=第三站&url=https://third.example&sort=ten&do=insert", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(tiebidli::friend_links::list(&db).await.unwrap().len(), 2);
}
