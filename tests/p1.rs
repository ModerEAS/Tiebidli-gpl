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
    boot_inner(None).await
}

async fn boot_theme() -> (axum::Router, Db) {
    boot_inner(Some(std::path::PathBuf::from("themes/handsome"))).await
}

async fn boot_inner(template_dir: Option<std::path::PathBuf>) -> (axum::Router, Db) {
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
    cfg.template_dir = template_dir;
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
    sqlx::query(
        r#"INSERT INTO "typecho_metas"
        ("mid","name","slug","type","description","count","order","parent")
        VALUES (1, '随笔', 'notes', 'category', '', 1, 0, 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(r#"INSERT INTO "typecho_relationships" ("cid","mid") VALUES (1, 1)"#)
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

    let log_count = || async {
        sqlx::query_scalar::<_, i64>(r#"SELECT COUNT(*) FROM "typecho_access_log""#)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    };
    let last_meta = || async {
        sqlx::query_scalar::<_, Option<i64>>(
            r#"SELECT meta_id FROM "typecho_access_log" ORDER BY id DESC LIMIT 1"#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };
    assert_eq!(log_count().await, 0);

    // 默认后端模式（writeType=0）：track.gif 只回像素、不写库，对齐 PHP `Action::writeLogs`。
    let (status, _, bytes, headers) = get(&app, "/access/log/track.gif?u=/archives/1/&cid=1&mid=0&rand=123").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("gif"));
    assert!(bytes.starts_with(b"GIF89a"), "{bytes:?}");
    assert_eq!(log_count().await, 0);

    // A2：后端模式覆盖全部 Archive。首页记一行，mid=0（对齐 parseArchive 的 index 分支）。
    let (status, home, _, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!home.contains("window.Access.track"), "{home}");
    assert_eq!(log_count().await, 1);
    assert_eq!(last_meta().await, Some(0));

    // A2：分类页记一行，mid=分类 mid。
    let (status, cat, _, _) = get(&app, "/category/notes/").await;
    assert_eq!(status, StatusCode::OK, "{cat}");
    assert_eq!(log_count().await, 2);
    assert_eq!(last_meta().await, Some(1));

    // A2：404 归档同样记一行。
    let (status, _, _, _) = get(&app, "/no-such-page-xyz").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(log_count().await, 3);

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
    // 后端模式下匿名文章页访问记一行日志（对齐 `Plugin::backend`）。
    assert_eq!(log_count().await, 4);

    // 管理员访问不记日志（对齐 PHP `writeLogs` 开头的 `isAdmin` return），浏览数照常按 cookie 去重。
    let (status, _, _, _) = call(
        &app,
        Request::builder().uri("/archives/1/").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(log_count().await, 4);
    assert_eq!(tiebidli::access::get_views(&db, 1).await, 2);

    // 切到前端写入后页脚注入打点。
    let (path, referer) = sign_action(&db, "/action/plugins-edit?config=Access").await;
    let (status, _, _) = post_form(&app, &path, "pageSize=10&isDrop=1&writeType=1&canAnalytize=1", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);

    // A1：页脚脚本带上 cid/mid。首页 mid=0，文章页 cid=1，分类页 mid=1。
    let (status, home, _, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("window.Access.track"), "{home}");
    assert!(home.contains("/access/log/track.gif"), "{home}");
    assert!(home.contains("mid=0"), "{home}");
    let (status, single, _, _) = get(&app, "/archives/1/").await;
    assert_eq!(status, StatusCode::OK, "{single}");
    assert!(single.contains("cid=1"), "{single}");
    assert!(single.contains("mid="), "{single}");
    let (status, cat, _, _) = get(&app, "/category/notes/").await;
    assert_eq!(status, StatusCode::OK, "{cat}");
    assert!(cat.contains("mid=1"), "{cat}");

    // 前端模式下 track.gif 写库。
    let (status, _, _, _) = get(&app, "/access/log/track.gif?u=/archives/1/&cid=1&mid=0&rand=456").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(log_count().await, 5);
    let cid: Option<i64> = sqlx::query_scalar(r#"SELECT content_id FROM "typecho_access_log" WHERE content_id = 1 LIMIT 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(cid, Some(1));

    // 管理员打点也不记。
    let (status, _, _, _) = call(
        &app,
        Request::builder().uri("/access/log/track.gif?u=/archives/1/&cid=1&mid=0&rand=789").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(log_count().await, 5);

    // 前端模式 + 去重 cookie：重访文章页既不加浏览数也不记日志。
    // （A1 复查那次无 cookie 访问已把 views 记到 3，这里带旧 cookie 重访不再加。）
    let (status, _, _, _) = call(
        &app,
        Request::builder().uri("/archives/1/").header("cookie", &views_cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tiebidli::access::get_views(&db, 1).await, 3);
    assert_eq!(log_count().await, 5);

    // B：控制台默认是访问日志 Tab。
    let (status, console, _, _) = call(
        &app,
        Request::builder().uri("/admin/access-console.php").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{console}");
    assert!(console.contains("访问日志"), "{console}");
    assert!(console.contains("访问概览"), "{console}");
    assert!(console.contains("受访地址"), "{console}");

    // B：访问概览 Tab 有今日/昨日/总计与来源、热门、小时表。
    let (status, overview, _, _) = call(
        &app,
        Request::builder().uri("/admin/access-console.php?action=overview").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    assert!(overview.contains("访问数表格"), "{overview}");
    assert!(overview.contains("今日"), "{overview}");
    assert!(overview.contains("昨日"), "{overview}");
    assert!(overview.contains("总计"), "{overview}");
    assert!(overview.contains("浏览量(PV)"), "{overview}");
    assert!(overview.contains("来源域名"), "{overview}");
    assert!(overview.contains("来源页"), "{overview}");
    assert!(overview.contains("热门内容"), "{overview}");
    assert!(overview.contains("今日图表"), "{overview}");

    // B：日志筛选（按 IP/文章/路由）可用。
    for path in [
        "/admin/access-console.php?action=logs&filter=ip&ip=0.0.0.0&type=3",
        "/admin/access-console.php?action=logs&filter=post&cid=1&type=3",
        "/admin/access-console.php?action=logs&filter=path&path=/&type=3",
    ] {
        let (status, page, _, _) = call(
            &app,
            Request::builder().uri(path).header("cookie", &cookie).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path} {page}");
        assert!(page.contains("取消筛选"), "{path}");
    }

    // B：delete.json 批量删除（管理员，无需 `_`，与 PHP 一致）。
    let ids: Vec<i64> =
        sqlx::query_scalar(r#"SELECT id FROM "typecho_access_log" ORDER BY id ASC LIMIT 2"#)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(ids.len(), 2);
    let (status, body, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/access/log/delete.json")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&ids).unwrap()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":0"), "{body}");
    assert_eq!(log_count().await, 3);
    // 非法体与未登录都回明确错误码。
    let (status, body, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/access/log/delete.json")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":100"), "{body}");
    let (status, body, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/access/log/delete.json")
            .header("content-type", "application/json")
            .body(Body::from("[1]"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Access Denied"), "{body}");

    // B：ip.json。本地可判定回 code 0，非法回明确错误；公网走 KeyCDN，
    // 可达回 code 0 的归属、不可达回 code 100 的 fallback 文案（均不断言具体网络结果）。
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/access/ip.json?ip=127.0.0.1").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":0"), "{body}");
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/access/ip.json?ip=bogus").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":100"), "{body}");
    assert!(body.contains("解析ip失败"), "{body}");
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/access/ip.json?ip=8.8.8.8").header("cookie", &cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("\"code\":0") || body.contains("tools.keycdn.com"),
        "{body}"
    );
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/access/ip.json?ip=127.0.0.1").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Access Denied"), "{body}");
}

#[tokio::test]
async fn access_is_drop_zero_drops_table_on_deactivate() {
    // `isDrop=0`（删除）时停用删日志表，`isDrop=1`（保留）时留表，与 PHP 配置一致。
    let (app, db) = boot().await;
    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Access").await;
    assert!(tiebidli::access::table_exists(&db).await);

    let (path, referer) = sign_action(&db, "/action/plugins-edit?config=Access").await;
    let (status, _, _) = post_form(&app, &path, "pageSize=10&isDrop=0&writeType=0&canAnalytize=1", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    let (path, referer) = sign_action(&db, "/action/plugins-edit?deactivate=Access").await;
    let (status, _, _) = post_form(&app, &path, "", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(!tiebidli::access::table_exists(&db).await);

    // 重新启用建回表；默认 `isDrop=1` 停用保留表。
    activate(&app, &db, &cookie, "Access").await;
    assert!(tiebidli::access::table_exists(&db).await);
    let (path, referer) = sign_action(&db, "/action/plugins-edit?deactivate=Access").await;
    let (status, _, _) = post_form(&app, &path, "", &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(tiebidli::access::table_exists(&db).await);
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

#[tokio::test]
async fn links_edit_requires_csrf_token() {
    // 与 Typecho 一样的门禁：`_` token 对不上就回 Referer 且不写库，不是数据不兼容。
    // 负向：无 `_` / 错误 `_` 时已登录、Referer 正确，insert 不得插入新行。
    // 正向：`sign_action` 带正确 `_` 时仍须插入成功。
    let (app, db) = boot().await;
    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Links").await;
    let referer = "http://test.local/admin/index.php";
    let form = "name=csrf站&url=https://csrf.example&sort=ten&image=&description=d&do=insert";
    let count = || async {
        tiebidli::friend_links::list(&db).await.unwrap().len()
    };
    assert_eq!(count().await, 0);

    // 无 `_`：302 回 Referer，库不变。
    let (status, _, headers) =
        post_form(&app, "/action/links-edit?do=insert", form, &cookie, referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get("location").and_then(|v| v.to_str().ok()).unwrap_or(""),
        referer
    );
    assert_eq!(count().await, 0);

    // 错误 `_`：同样不写库。
    let (status, _, _) = post_form(
        &app,
        "/action/links-edit?do=insert&_=badtoken",
        form,
        &cookie,
        referer,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(count().await, 0);

    // 正确 `_`：插入成功（正向对照）。
    let (path, signed_referer) = sign_action(&db, "/action/links-edit?do=insert").await;
    let (status, _, headers) = post_form(&app, &path, form, &cookie, signed_referer).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(headers.get("location").unwrap().to_str().unwrap().contains("notice="));
    assert_eq!(count().await, 1);
}

#[tokio::test]
async fn access_feed_entrypoint_ua_parity() {
    // 对齐 PHP `Widget_Archive->beforeRender` / `Access_Core::getEntryPoint` / UA 入库。
    let (app, db) = boot().await;
    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Access").await;
    let ep_name = tiebidli::auth::cookie_name("http://test.local", tiebidli::access::ENTRYPOINT_COOKIE);
    let log_count = || async {
        sqlx::query_scalar::<_, i64>(r#"SELECT COUNT(*) FROM "typecho_access_log""#)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    };
    async fn last_ep(db: &Db) -> (String, i64, String, String) {
        let row = sqlx::query(r#"SELECT entrypoint, robot, robot_id, browser_id FROM "typecho_access_log" ORDER BY id DESC LIMIT 1"#)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        (
            tiebidli::db::col_str(&row, "entrypoint"),
            tiebidli::db::col_i64(&row, "robot").unwrap_or(-1),
            tiebidli::db::col_str(&row, "robot_id"),
            tiebidli::db::col_str(&row, "browser_id"),
        )
    }
    fn set_cookies(headers: &axum::http::HeaderMap) -> String {
        headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join("\n")
    }
    assert_eq!(log_count().await, 0);

    // 1) feed 归档（后端模式）记一行，cid/mid 为空。
    let (status, feed, _, _) = get(&app, "/feed/").await;
    assert_eq!(status, StatusCode::OK, "{feed}");
    assert!(feed.contains("<rss") || feed.contains("<feed") || feed.contains("<rdf"), "{feed}");
    assert_eq!(log_count().await, 1);
    let nulls: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM "typecho_access_log" WHERE content_id IS NULL AND meta_id IS NULL"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(nulls, 1);
    // 评论 feed 同样记。
    let (status, _, _, _) = get(&app, "/feed/comments/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(log_count().await, 2);

    // 2) 外链 Referer 首次访问：入库 entrypoint + Set-Cookie。
    let (status, _, _, headers) = call(
        &app,
        Request::builder().uri("/").header("referer", "https://ext.example/a").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(log_count().await, 3);
    let set = set_cookies(&headers);
    assert!(set.contains(&ep_name), "{set}");
    let (ep, _, _, _) = last_ep(&db).await;
    assert_eq!(ep, "https://ext.example/a");
    let ep_cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{ep_name}=")))
        .map(|v| v.split(';').next().unwrap_or("").to_string())
        .expect("entrypoint cookie");

    // 同站 Referer：entrypoint 置空且不写 cookie（旧 cookie 保留不断言）。
    let (status, _, _, headers) = call(
        &app,
        Request::builder()
            .uri("/")
            .header("referer", "http://test.local/archives/1/")
            .header("cookie", &ep_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (ep, _, _, _) = last_ep(&db).await;
    assert_eq!(ep, "");
    assert!(!set_cookies(&headers).contains(&ep_name));

    // 无 Referer + 带 cookie：沿用 cookie 的 entrypoint，不重复写。
    let (status, _, _, headers) = call(
        &app,
        Request::builder().uri("/").header("cookie", &ep_cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (ep, _, _, _) = last_ep(&db).await;
    assert_eq!(ep, "https://ext.example/a");
    assert!(!set_cookies(&headers).contains(&ep_name));

    // 3) 爬虫 UA：robot=1 且 robot_id 非空。
    let (status, _, _, _) = call(
        &app,
        Request::builder()
            .uri("/")
            .header("user-agent", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, robot, robot_id, _) = last_ep(&db).await;
    assert_eq!(robot, 1);
    assert!(!robot_id.is_empty(), "{robot_id}");

    // 普通浏览器：browser_id/os 有值，robot=0。
    let (status, _, _, _) = call(
        &app,
        Request::builder()
            .uri("/")
            .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, robot, _, browser) = last_ep(&db).await;
    assert_eq!(robot, 0);
    assert_eq!(browser, "Chrome");
}

#[tokio::test]
async fn theme_parity_layout_and_actions() {
    // 新主题骨架：THEME_DIR 指向 themes/handsome，断言现网对齐的 class/节点。
    let (app, db) = boot_theme().await;
    let cookie = login(&app).await;
    activate(&app, &db, &cookie, "Access").await;
    activate(&app, &db, &cookie, "Links").await;
    sqlx::query(r#"INSERT INTO "typecho_links" ("name","url","sort","image","description","user","order") VALUES ('bobo','https://example.com','ten','','','',0)"#)
        .execute(&db.pool)
        .await
        .unwrap();

    let (status, home, _, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    for needle in [
        "blog-post post-list",
        "single-post panel",
        "index-post-title",
        "post-item-foot",
        "浏览次数",
        "post-calendar",
        "category-radar",
        "widget-tabs-4-hots",
        "handsome_global_player",
        "MUSIC_API",
        "action/handsome-meting-api",
        "使用links插件",
        "bobo",
        "app-header",
        "app-aside",
        "app-content-body",
        "app-footer",
        "search_modal",
    ] {
        assert!(home.contains(needle), "missing {needle}");
    }

    // 文章页：正文容器、目录、朗读/阅读模式、点赞按钮与计数。
    let (status, single, _, _) = get(&app, "/archives/1/").await;
    assert_eq!(status, StatusCode::OK, "{single}");
    for needle in [
        "entry-content",
        "tocTree",
        "star_post",
        "like_label",
        "post_speech",
        "read_mode_toggle",
        "respond-post-1",
    ] {
        assert!(single.contains(needle), "missing {needle}");
    }

    // 点赞：成功 1，带 cookie 重复 2，不存在回 -1。
    let (status, body, _, headers) = call(
        &app,
        Request::builder().method("POST").uri("/?action=star_post&cid=1").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.trim(), "1");
    let star_cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(star_cookie.contains("extend_post_stars"), "{star_cookie}");
    let (status, body, _, _) = call(
        &app,
        Request::builder().method("POST").uri("/?action=star_post&cid=1").header("cookie", &star_cookie).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.trim(), "2");
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/?action=star_post&cid=99999").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.trim(), "-1");
    // star_talk 缺参数/无行同样 -1。
    let (status, body, _, _) = call(
        &app,
        Request::builder().uri("/?action=star_talk&coid=99999").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.trim(), "-1");

    // 音乐 Action：非法源/缺参 403；parse 未登录 403。
    for uri in [
        "/action/handsome-meting-api?server=spotify&type=song&id=1",
        "/action/handsome-meting-api?server=netease&type=song&id=",
        "/action/handsome-meting-api?do=parse&data=https://music.163.com/playlist?id=1",
    ] {
        let (status, _, _, _) = call(&app, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    // 时光机编辑：匿名 100。
    let (status, body, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/action/cross-edit?do=edit&coid=1")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("text=hi"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":100"), "{body}");
}

#[tokio::test]
async fn cross_edit_authed_flow() {
    // 登录作者改自己的时光机评论：token 绑定来源页，成功 code 0 并落库。
    let (app, db) = boot_theme().await;
    sqlx::query(
        r#"INSERT INTO "typecho_contents"
        ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
        VALUES (100, '时光机', 'cross', 1700000000, 1700000100, 'x', 0, 1, NULL, 'page', 'publish', '', 0, '1', '1', '1', 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let coid: i64 = sqlx::query(
        r#"INSERT INTO "typecho_comments" ("cid","created","author","authorId","ownerId","mail","url","ip","agent","text","type","status","parent")
        VALUES (100, 1700000000, '站长', 1, 1, '', '', '127.0.0.1', '', '旧内容', 'comment', 'approved', 0) RETURNING coid"#,
    )
    .fetch_one(&db.pool)
    .await
    .map(|row| tiebidli::db::col_i64(&row, "coid").unwrap())
    .unwrap_or(1);
    let cookie = login(&app).await;
    let code = tiebidli::auth::security_token(
        "test-secret",
        Some((auth_code(&db).await.as_deref().unwrap_or(""), 1)),
        "http://test.local/cross.html",
    );
    let uri = format!("/action/cross-edit?do=edit&coid={coid}&_={code}");
    let (status, body, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(&uri)
            .header("cookie", &cookie)
            .header("referer", "http://test.local/cross.html")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("text=新内容"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"code\":0"), "{body}");
    // 时光机页渲染评论流（含 star_talk 与编辑入口）。
    let (status, cross, _, _) = get(&app, "/cross.html").await;
    assert_eq!(status, StatusCode::OK, "{cross}");
    for needle in ["cross_stream", "star_talk", "新内容"] {
        assert!(cross.contains(needle), "missing {needle}");
    }
}
