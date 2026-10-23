use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tiebidli::db::Db;
use tiebidli::{build_router, AppState, Config};
use tower::ServiceExt;

static N: AtomicU64 = AtomicU64::new(0);

async fn boot() -> (axum::Router, Db, std::path::PathBuf) {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "tiebidli-{}-{}-{n}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let mut cfg = Config::for_test(&url);
    let uploads = path.with_extension("uploads");
    std::fs::create_dir_all(&uploads).expect("uploads");
    cfg.uploads_dir = Some(uploads.clone());
    let themes = path.with_extension("themes");
    let theme_dir = themes.join("default");
    std::fs::create_dir_all(theme_dir.join("assets/css")).expect("theme marker dir");
    std::fs::write(theme_dir.join("assets/css/style.css"), "/* test */").expect("theme marker");
    cfg.theme_dir = theme_dir;
    let backups = path.with_extension("backups");
    std::fs::create_dir_all(&backups).expect("backups");
    cfg.backup_dir = backups;
    let templates = path.with_extension("templates");
    std::fs::create_dir_all(&templates).expect("templates");
    cfg.template_dir = Some(templates);
    let db = Db::connect(&url, &cfg.prefix).await.expect("connect");
    for stmt in include_str!("../schema/typecho-1.2-sqlite.sql").split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt)
            .execute(&db.pool)
            .await
            .unwrap_or_else(|e| panic!("schema: {e}\n{stmt}"));
    }
    seed(&db).await;
    let app = build_router(Arc::new(AppState::new(db.clone(), cfg)));
    (app, db, uploads)
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

    let posts = [
        (
            1_i64,
            "你好世界",
            "hello-world",
            1_700_000_000_i64,
            "<!--markdown-->第一段\n\n<!--more-->\n\n秘密段落",
            "publish",
            "",
            1_i64,
            "post",
        ),
        (
            2,
            "第二篇",
            "second-post",
            1_700_001_000,
            "<!--markdown-->第二篇正文",
            "publish",
            "",
            0,
            "post",
        ),
        (
            3,
            "密码文",
            "secret-post",
            1_690_000_000,
            "<!--markdown-->密码正文",
            "publish",
            "s3cret",
            0,
            "post",
        ),
        (
            4,
            "隐藏文",
            "hidden-post",
            1_700_000_500,
            "<!--markdown-->隐藏正文",
            "hidden",
            "",
            0,
            "post",
        ),
        (
            5,
            "未来文",
            "future-post",
            4_000_000_000,
            "<!--markdown-->还没发布",
            "publish",
            "",
            0,
            "post",
        ),
        (
            6,
            "关于",
            "about",
            1_700_000_000,
            "<!--markdown-->关于本站",
            "publish",
            "",
            0,
            "page",
        ),
    ];
    for (cid, title, slug, created, text, status, password, comments, kind) in posts {
        let allow_feed = if cid == 3 { "0" } else { "1" };
        sqlx::query(
            r#"INSERT INTO "typecho_contents"
            ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
            VALUES (?, ?, ?, ?, ?, ?, 0, 1, NULL, ?, ?, ?, ?, '1', '1', ?, 0)"#,
        )
        .bind(cid)
        .bind(title)
        .bind(slug)
        .bind(created)
        .bind(created)
        .bind(text)
        .bind(kind)
        .bind(status)
        .bind(password)
        .bind(comments)
        .bind(allow_feed)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    sqlx::query(
        r#"INSERT INTO "typecho_metas"
        ("mid","name","slug","type","description","count","order","parent")
        VALUES (1, '随笔', 'notes', 'category', '随笔分类', 2, 0, 0),
               (2, 'Rust', 'rust', 'tag', '', 1, 0, 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO "typecho_relationships" ("cid","mid") VALUES (1, 1), (1, 2), (2, 1)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO "typecho_fields" ("cid","name","type","str_value","int_value","float_value")
           VALUES (2, 'customSummary', 'str', '自定义摘要', 0, 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO "typecho_comments"
        ("cid","created","author","authorId","ownerId","mail","url","ip","agent","text","type","status","parent")
        VALUES (1, 1700000100, '旧读者', 0, 1, 'old@example.com', NULL, '127.0.0.1', 'test', '早已在此', 'comment', 'approved', 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let theme = r#"a:2:{s:8:"BlogName";s:6:"测试";s:7:"BlogJob";s:12:"写点东西";}"#;
    let options = [
        ("title", "测试博客"),
        ("description", "一句话描述"),
        ("keywords", "rust"),
        ("siteUrl", "http://test.local"),
        ("pageSize", "1"),
        ("timezone", "0"),
        ("frontPage", "recent"),
        ("frontArchive", "0"),
        ("postDateFormat", "Y-m-d"),
        ("feedFullText", "1"),
        ("commentsRequireMail", "1"),
        ("commentsRequireURL", "0"),
        ("commentsRequireModeration", "0"),
        ("commentsWhitelist", "0"),
        ("commentsCheckReferer", "1"),
        ("commentsPostIntervalEnable", "0"),
        ("commentsPostInterval", "60"),
        ("commentsMarkdown", "0"),
        ("commentsThreaded", "1"),
        ("rewrite", "1"),
        ("secret", "test-secret"),
        ("theme:default", theme),
    ];
    for (name, value) in options {
        sqlx::query(r#"INSERT INTO "typecho_options" ("name","user","value") VALUES (?, 0, ?)"#)
            .bind(name)
            .bind(value)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

async fn call(
    app: &axum::Router,
    req: Request<Body>,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let res = app.clone().oneshot(req).await.expect("oneshot");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        headers,
    )
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, String, axum::http::HeaderMap) {
    call(
        app,
        Request::builder().uri(path).body(Body::empty()).unwrap(),
    )
    .await
}

async fn post(
    app: &axum::Router,
    path: &str,
    form: &str,
    referer: Option<&str>,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(referer) = referer {
        b = b.header("referer", referer);
    }
    call(app, b.body(Body::from(form.to_string())).unwrap()).await
}

fn assert_ok(label: &str, status: StatusCode, body: &str) {
    assert_eq!(status, StatusCode::OK, "{label}\n{body}");
}

#[tokio::test]
async fn public_pages_follow_typecho_12() {
    let (app, db, _uploads) = boot().await;

    let (status, body, _) = get(&app, "/").await;
    assert_ok("home", status, &body);
    assert!(body.contains("第二篇"), "{body}");
    assert!(body.contains("自定义摘要"), "{body}");
    assert!(body.contains("id=\"nav-menu\""), "{body}");
    assert!(body.contains("id=\"secondary\""), "{body}");
    assert!(body.contains("最新文章"), "{body}");
    assert!(body.contains("/page/2/"), "{body}");
    assert!(body.contains("post-title"), "{body}");
    assert!(body.contains("/usr/themes/default/assets/css/style.css"), "{body}");
    assert!(!body.contains("隐藏文"), "{body}");
    assert!(!body.contains("未来文"), "{body}");

    let (status, body, _) = get(&app, "/page/2/").await;
    assert_ok("page2", status, &body);
    assert!(body.contains("你好世界"), "{body}");
    assert!(body.contains("第一段"), "{body}");
    assert!(!body.contains("秘密"), "{body}");

    let (status, body, _) = get(&app, "/page/3/").await;
    assert_ok("page3", status, &body);
    assert!(body.contains("此内容被密码保护"), "{body}");

    let (status, body, _) = get(&app, "/page/4/").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body, _) = get(&app, "/archives/1/").await;
    assert_ok("post cid", status, &body);
    assert!(body.contains("秘密段落") || body.contains("秘密"), "{body}");
    assert!(body.contains("早已在此"), "{body}");
    assert!(body.contains("上一篇"), "{body}");
    assert!(body.contains("第二篇"), "{body}");
    assert!(body.contains("TypechoComment.reply"), "{body}");
    assert!(body.contains("cancel-comment-reply-link"), "{body}");
    assert!(body.contains("TypechoComment.cancelReply"), "{body}");
    assert!(body.contains("id=\"comment-parent\""), "{body}");

    let (status, _, headers) = get(&app, "/archives/hello-world").await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY, "slug falls back to the cid permalink");
    assert_eq!(headers.get("location").unwrap().to_str().unwrap(), "/archives/1/");
    let (status, _, headers) = get(&app, "/archives/hello-world.html").await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(headers.get("location").unwrap().to_str().unwrap(), "/archives/1/");
    let (status, _, headers) = get(&app, "/index.php/archives/1/").await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY, "rewrite on canonicalizes away from index.php");
    assert_eq!(headers.get("location").unwrap().to_str().unwrap(), "/archives/1/");
    let (status, body, _) = get(&app, "/index.php").await;
    assert_ok("index.php home", status, &body);
    assert!(body.contains("第二篇"), "{body}");
    assert!(!body.contains("/index.php/archives/"), "{body}");

    let (status, body, _) = get(&app, "/archives/2/").await;
    assert_ok("newer", status, &body);
    assert!(body.contains("下一篇"), "{body}");
    assert!(body.contains("你好世界"), "{body}");

    let (status, body, _) = get(&app, "/category/notes/").await;
    assert_ok("category", status, &body);
    assert!(body.contains("第二篇"), "{body}");
    assert!(body.contains("随笔"), "{body}");
    let (status, body, _) = get(&app, "/tag/rust/").await;
    assert_ok("tag", status, &body);
    assert!(body.contains("你好世界"), "{body}");

    let (status, body, _) = get(&app, "/search/%E7%A7%98%E5%AF%86/").await;
    assert_ok("search", status, &body);
    assert!(body.contains("你好世界"), "{body}");
    assert!(!body.contains("密码正文"), "{body}");

    let (status, _, headers) = get(&app, "/search/?s=%E7%A7%98%E5%AF%86").await;
    assert_eq!(status, StatusCode::FOUND);
    let loc = headers.get("location").unwrap().to_str().unwrap();
    assert!(loc.contains("/search/"), "{loc}");

    let (status, body, headers) = get(&app, "/feed/").await;
    assert_ok("feed", status, &body);
    assert!(headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("rss"));
    assert!(
        body.contains("你好世界") && body.contains("第二篇"),
        "{body}"
    );
    assert!(
        !body.contains("密码文") && !body.contains("隐藏文") && !body.contains("未来文"),
        "{body}"
    );

    let (status, body, headers) = get(&app, "/feed/atom/").await;
    assert_ok("atom", status, &body);
    assert!(headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("atom"));
    assert!(body.contains("<feed"), "{body}");

    let (status, body, _) = get(&app, "/feed/archives/1/").await;
    assert_ok("post feed", status, &body);
    assert!(body.contains("早已在此"), "{body}");

    let (status, body, _) = get(&app, "/about.html").await;
    assert_ok("page", status, &body);
    assert!(body.contains("关于本站"), "{body}");

    let (status, body, _) = get(&app, "/author/1/").await;
    assert_ok("author", status, &body);
    assert!(body.contains("站长"), "{body}");
    assert!(body.contains("第二篇"), "{body}");

    let (status, body, _) = get(&app, "/2023/11/14/").await;
    assert_ok("day", status, &body);
    assert!(body.contains("第二篇"), "{body}");

    let (status, body, _) = get(&app, "/archives/4/").await;
    assert_ok("hidden", status, &body);
    assert!(body.contains("隐藏正文"), "{body}");

    let (status, body, _) = get(&app, "/archives/5/").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body, _) = get(&app, "/no-such").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body.contains("404"), "{body}");

    let (status, body, _) = get(&app, "/blog/").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "blog archive {body}");

    let (status, body, _) = get(&app, "/archives/3/").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("请输入密码访问"), "{body}");
    assert!(!body.contains("密码正文"), "{body}");

    let (status, body, _) = post(
        &app,
        "/archives/3/",
        "protectPassword=nope&protectCID=3",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("密码错误"), "{body}");

    let (status, body, headers) = post(
        &app,
        "/archives/3/",
        "protectPassword=s3cret&protectCID=3",
        None,
    )
    .await;
    assert_ok("unlock", status, &body);
    assert!(body.contains("密码正文"), "{body}");
    assert!(headers.get("set-cookie").is_some());
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join(";");
    let prefix = tiebidli::auth::cookie_prefix("http://test.local");
    assert!(
        cookie.contains(&format!("{prefix}protectPassword_3")),
        "{cookie}"
    );

    let (status, body, _) = post(
        &app,
        "/archives/1/comment",
        "author=&mail=a@b.co&text=你好评论&parent=0",
        Some("http://test.local/archives/1/"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("必须填写用户名"), "{body}");

    let (status, body, _) = post(
        &app,
        "/archives/1/comment",
        "author=站长&mail=a@b.co&text=你好评论&parent=0",
        Some("http://test.local/archives/1/"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("已经被注册"), "{body}");

    let (status, _, headers) = post(
        &app,
        "/archives/1/comment",
        "author=路人&mail=a@b.co&text=你好评论&parent=0",
        Some("http://test.local/archives/1/"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "comment redirect");
    let loc = headers.get("location").unwrap().to_str().unwrap();
    assert!(loc.contains("#comment-"), "{loc}");
    let (status, body, _) = get(&app, "/archives/1/").await;
    assert_ok("comment visible", status, &body);
    assert!(body.contains("你好评论"), "{body}");

    sqlx::query(
        r#"UPDATE "typecho_options" SET value = ? WHERE name = 'commentsPostIntervalEnable'"#,
    )
    .bind("1")
    .execute(&db.pool)
    .await
    .unwrap();
    let ten_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 10;
    sqlx::query(r#"UPDATE "typecho_comments" SET created = ? WHERE cid = 1"#)
        .bind(ten_ago)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, body, _) = post(
        &app,
        "/archives/1/comment",
        "author=路人乙&mail=b@b.co&text=太快了&parent=0",
        Some("http://test.local/archives/1/"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("过于频繁"), "{body}");

    let route = r#"a:1:{s:4:"post";a:1:{s:3:"url";s:21:"/archives/[slug].html";}}"#;
    sqlx::query(
        r#"INSERT INTO "typecho_options" ("name","user","value") VALUES ('routingTable', 0, ?)"#,
    )
    .bind(route)
    .execute(&db.pool)
    .await
    .unwrap();
    let (status, body, _) = get(&app, "/archives/hello-world.html").await;
    assert_ok("custom permalink", status, &body);
    assert!(
        body.contains("hello-world.html"),
        "custom permalink missing from rendered links"
    );
    let (status, body, _) = get(&app, "/archives/1/").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cid permalink retired\n{body}"
    );

    sqlx::query(r#"UPDATE "typecho_options" SET value = 'page:6' WHERE name = 'frontPage'"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, body, _) = get(&app, "/").await;
    assert_ok("static front", status, &body);
    assert!(body.contains("关于本站"), "{body}");
    let (status, body, _) = get(&app, "/page/2/").await;
    assert_ok("static front page 2", status, &body);
    assert!(body.contains("关于本站"), "{body}");
    let (status, _, _) = get(&app, "/blog/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    sqlx::query(r#"UPDATE "typecho_options" SET value = '1' WHERE name = 'frontArchive'"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, body, _) = get(&app, "/blog/").await;
    assert_ok("front archive", status, &body);
    assert!(body.contains("第二篇"), "{body}");
}

#[tokio::test]
async fn feeds_comments_admin_and_xmlrpc() {
    let (app, db, uploads) = boot().await;
    let prefix = tiebidli::auth::cookie_prefix("http://test.local");

    let (status, body, headers) = get(&app, "/feed/rss/").await;
    assert_ok("rss1", status, &body);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("rdf"),
        "{headers:?}"
    );
    assert!(body.contains("rdf:RDF"), "{body}");
    assert!(body.contains("你好世界"), "{body}");

    let (status, body, _) = get(&app, "/feed/atom/").await;
    assert_ok("atom still", status, &body);
    assert!(body.contains("<feed"), "{body}");

    sqlx::query(
        r#"INSERT INTO "typecho_options" ("name","user","value") VALUES
           ('commentsPageBreak', 0, '1'),
           ('commentsPageSize', 0, '1')"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO "typecho_comments"
        ("cid","created","author","authorId","ownerId","mail","url","ip","agent","text","type","status","parent")
        VALUES (1, 1700000200, '后来的人', 0, 1, 'new@example.com', NULL, '127.0.0.1', 'test', '第二页评论', 'comment', 'approved', 0)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let (status, body, _) = get(&app, "/archives/1/comment-page-2").await;
    assert_ok("comment page", status, &body);
    assert!(body.contains("第二页评论"), "{body}");
    assert!(body.contains("comment-page-nav"), "{body}");
    assert!(body.contains("class=\"current\""), "{body}");

    for (cid, title, status_name) in [(21_i64, "私人文章", "private"), (22, "草稿文章", "draft")] {
        sqlx::query(
            r#"INSERT INTO "typecho_contents"
            ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
            VALUES (?, ?, ?, 1700000000, 1700000000, '不可见', 0, 1, NULL, 'post', ?, NULL, 0, '1', '1', '1', 0)"#,
        )
        .bind(cid)
        .bind(title)
        .bind(format!("hidden-{cid}"))
        .bind(status_name)
        .execute(&db.pool)
        .await
        .unwrap();
        let (status, body, _) = get(&app, &format!("/archives/{cid}/")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{title} {body}");
    }
    let (status, body, _) = get(&app, "/").await;
    assert_ok("home hides private", status, &body);
    assert!(!body.contains("私人文章") && !body.contains("草稿文章"), "{body}");

    std::fs::write(uploads.join("note.txt"), b"file-body").unwrap();
    let attach = tiebidli::write::attachment_text(
        "note.txt",
        "usr/uploads/note.txt",
        9,
        "txt",
        "text/plain",
    );
    sqlx::query(
        r#"INSERT INTO "typecho_contents"
        ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
        VALUES (30, '附件说明', 'attach-note', 1700000000, 1700000000, ?, 0, 1, NULL, 'attachment', 'publish', NULL, 0, '0', '0', '0', 0)"#,
    )
    .bind(attach)
    .execute(&db.pool)
    .await
    .unwrap();
    let (status, body, _) = get(&app, "/attachment/30/").await;
    assert_ok("attachment", status, &body);
    assert!(body.contains("note.txt"), "{body}");
    assert!(body.contains("?download=1"), "{body}");
    assert!(!body.contains("评论已关闭"), "{body}");
    let (status, body, headers) = get(&app, "/attachment/30/?download=1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("file-body"), "{body}");
    assert!(
        headers
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("note.txt"),
        "{headers:?}"
    );

    let (status, body, _) = post(
        &app,
        "/archives/1/trackback",
        "blog_name=Example&url=http://example.com/post&excerpt=引用摘要",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("<success>0</success>"), "{body}");
    assert!(body.contains("Trackback has registered."), "{body}");
    let (status, body, _) = get(&app, "/archives/1/").await;
    assert_ok("ping list", status, &body);
    assert!(body.contains("引用摘要"), "{body}");
    assert!(body.contains("ping-list"), "{body}");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = [0u8; 2048];
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = sock.read(&mut buf).await;
        let page = "<html><title>来源页</title><a href=\"http://test.local/archives/1/\">链接</a></html>";
        let resp = format!(
            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let source = format!("http://127.0.0.1:{port}/source");
    let ping_xml = format!(
        r#"<?xml version="1.0"?><methodCall><methodName>pingback.ping</methodName><params><param><value><string>{source}</string></value></param><param><value><string>http://test.local/archives/1/</string></value></param></params></methodCall>"#
    );
    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/action/xmlrpc")
            .header("content-type", "text/xml")
            .body(Body::from(ping_xml))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("fault"), "{body}");

    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();
    let xml = r#"<?xml version="1.0"?><methodCall><methodName>metaWeblog.newPost</methodName><params><param><value><string>1</string></value></param><param><value><string>admin</string></value></param><param><value><string>adminpass</string></value></param><param><value><struct><member><name>title</name><value><string>接口文章</string></value></member><member><name>description</name><value><string>接口正文</string></value></member><member><name>dateCreated</name><value><dateTime.iso8601>20200101T00:00:00</dateTime.iso8601></value></member></struct></value></param><param><value><boolean>1</boolean></value></param></params></methodCall>"#;
    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/action/xmlrpc")
            .header("content-type", "text/xml")
            .body(Body::from(xml))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("<int>"), "{body}");
    let cid = body
        .split("<int>")
        .nth(1)
        .and_then(|s| s.split("</int>").next())
        .unwrap_or("0");
    let (status, body, _) = get(&app, &format!("/archives/{cid}/")).await;
    assert_ok("xmlrpc post", status, &body);
    assert!(body.contains("接口正文"), "{body}");

    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(cookie.contains(&format!("{prefix}__typecho_uid")), "{cookie}");
    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/contents-post-edit?cid=2",
        &cookie,
        "title=改过的标题&slug=second-post&text=修改后的正文&status=publish&created=1700001000&allowComment=on&allowPing=on&allowFeed=on",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "{body}");
    let (status, body, _) = get(&app, "/archives/2/").await;
    assert_ok("edited post", status, &body);
    assert!(body.contains("改过的标题"), "{body}");
    assert!(body.contains("修改后的正文"), "{body}");
}

#[tokio::test]
async fn admin_profile_comments_preview_and_fields() {
    let (app, db, _uploads) = boot().await;
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO "typecho_users"
        ("uid","name","password","mail","url","screenName","created","activated","logged","group","authCode")
        VALUES (2, 'other', 'x', 'other@example.com', '', '另一人', 1600000000, 1, 0, 'subscriber', NULL)"#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin", &cookie, "").await;
    assert_ok("dashboard author link", status, &body);
    assert!(body.contains("href=\"/admin/profile.php\""), "{body}");
    assert!(body.contains("个人设置"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/profile.php", &cookie, "").await;
    assert_ok("profile", status, &body);
    assert!(body.contains("个人资料"), "{body}");
    assert!(body.contains("撰写设置"), "{body}");
    assert!(body.contains("密码修改"), "{body}");
    assert!(!body.contains("插件个人设置"), "{body}");
    assert!(body.contains("更新我的档案"), "{body}");
    assert!(body.contains("使用 Markdown 语法编辑和解析内容"), "{body}");
    assert!(body.contains("可以被评论"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=站长&url=https://example.com&mail=&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("empty mail", status, &body);
    assert!(body.contains("必须填写电子邮箱"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=站长&url=https://example.com&mail=not-an-email&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("bad mail", status, &body);
    assert!(body.contains("电子邮箱格式错误"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=%3Cb%3Ex%3C%2Fb%3E&url=https://example.com&mail=admin@example.com&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("xss screen", status, &body);
    assert!(body.contains("请不要在昵称中使用特殊字符"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=%E7%AB%99%E9%95%BF&url=javascript:alert(1)&mail=admin@example.com&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("bad url", status, &body);
    assert!(body.contains("个人主页地址格式错误"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=%E5%8F%A6%E4%B8%80%E4%BA%BA&url=https://example.com&mail=admin@example.com&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("dup screen", status, &body);
    assert!(body.contains("昵称已经存在"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "screenName=%E6%96%B0%E7%AB%99%E9%95%BF&url=https://example.com/me&mail=admin@example.com&do=profile",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("profile saved", status, &body);
    assert!(body.contains("您的档案已经更新"), "{body}");
    assert!(body.contains("value=\"新站长\""), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "markdown=1&xmlrpcMarkdown=0&autoSave=1&defaultAllow%5B%5D=comment&do=options",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("options saved", status, &body);
    assert!(body.contains("设置已经保存"), "{body}");
    let markdown: String = sqlx::query_scalar(
        r#"SELECT value FROM "typecho_options" WHERE name = 'markdown' AND user = 1"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(markdown, "1");
    let ping: String = sqlx::query_scalar(
        r#"SELECT value FROM "typecho_options" WHERE name = 'defaultAllowPing' AND user = 1"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ping, "0");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "password=short&confirm=short&do=password",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("short password", status, &body);
    assert!(body.contains("为了保证账户安全, 请输入至少六位的密码"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "password=longpass&confirm=otherpass&do=password",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("password mismatch", status, &body);
    assert!(body.contains("两次输入的密码不一致"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-profile",
        &cookie,
        "password=longpass&confirm=longpass&do=password",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (status, body, _) = authed(&app, &db, "GET", &location_of(&headers), &cookie, "").await;
    assert_ok("password saved", status, &body);
    assert!(body.contains("密码已经成功修改"), "{body}");
    let (status, headers) = login_as(&app, "admin", "longpass").await;
    assert_eq!(status, StatusCode::FOUND, "new password");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-comments.php", &cookie, "").await;
    assert_ok("comments", status, &body);
    assert!(body.contains("class=\"operate-edit\""), "{body}");
    assert!(body.contains("class=\"operate-reply\""), "{body}");
    assert!(body.contains("data-comment="), "{body}");
    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/comments-edit?do=edit&coid=1",
        &cookie,
        "author=%E6%94%B9%E4%BD%9C%E8%80%85&mail=new%40example.com&url=https%3A%2F%2Fex.com&text=%E6%94%B9%E8%BF%87%E7%9A%84%E8%AF%84%E8%AE%BA",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"success\":1"), "{body}");
    assert!(body.contains("改过的评论"), "{body}");
    let text: String = sqlx::query_scalar(r#"SELECT text FROM "typecho_comments" WHERE coid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(text, "改过的评论");

    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/comments-edit?do=reply&coid=1",
        &cookie,
        "text=%E7%AE%A1%E7%90%86%E5%91%98%E5%9B%9E%E5%A4%8D",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"success\":1"), "{body}");
    let (parent, author): (i64, String) = sqlx::query_as(
        r#"SELECT parent, author FROM "typecho_comments" WHERE text = '管理员回复'"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(parent, 1);
    assert_eq!(author, "新站长");

    let before: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_contents""#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/admin/preview",
        &cookie,
        "title=%E9%A2%84%E8%A7%88%E6%9C%AA%E4%BF%9D%E5%AD%98%E6%A0%87%E9%A2%98&text=%E9%A2%84%E8%A7%88%E6%9C%AA%E4%BF%9D%E5%AD%98%E6%AD%A3%E6%96%87XYZ&slug=not-saved&type=post",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("预览未保存标题"), "{body}");
    assert!(body.contains("预览未保存正文XYZ"), "{body}");
    assert!(body.contains("id=\"nav-menu\""), "{body}");
    let after: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_contents""#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let (status, home, _) = get(&app, "/").await;
    assert_ok("home unchanged", status, &home);
    assert!(!home.contains("预览未保存标题"), "{home}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/write-post.php", &cookie, "").await;
    assert_ok("write form", status, &body);
    assert!(body.contains("预览文章"), "{body}");
    assert!(body.contains("自定义字段"), "{body}");
    assert!(body.contains("引用通告"), "{body}");
    assert!(body.contains("name=\"trackback\""), "{body}");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/contents-post-edit?cid=2",
        &cookie,
        "title=%E7%AC%AC%E4%BA%8C%E7%AF%87&slug=second-post&text=%E6%AD%A3%E6%96%87&do=publish&visibility=publish&created=1700001000&allowComment=1&allowPing=1&allowFeed=1&fieldNames%5B%5D=customSummary&fieldTypes%5B%5D=str&fieldValues%5B%5D=%E6%91%98%E8%A6%81%E4%B9%99&fieldNames%5B%5D=score&fieldTypes%5B%5D=int&fieldValues%5B%5D=7",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let summary: String = sqlx::query_scalar(
        r#"SELECT str_value FROM "typecho_fields" WHERE cid = 2 AND name = 'customSummary'"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(summary, "摘要乙");
    let score: i64 = sqlx::query_scalar(
        r#"SELECT int_value FROM "typecho_fields" WHERE cid = 2 AND name = 'score'"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(score, 7);
}

#[tokio::test]
async fn admin_category_edit_merge_and_media() {
    let (app, db, uploads) = boot().await;
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-categories.php", &cookie, "").await;
    assert_ok("category list", status, &body);
    assert!(body.contains("合并到"), "{body}");
    assert!(body.contains("子分类"), "{body}");
    assert!(body.contains("缩略名"), "{body}");
    assert!(body.contains("href=\"/admin/category.php?mid=1\""), "{body}");
    assert!(body.contains("href=\"/admin/category.php\""), "{body}");

    let (status, body, _) = authed(&app, &db, "POST", "/action/metas-category-edit", &cookie, "name=&slug=&parent=0&do=insert").await;
    assert_ok("empty category name", status, &body);
    assert!(body.contains("必须填写分类名称"), "{body}");

    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!("name={}&slug=dup-notes&parent=0&do=insert", enc("随笔")),
    )
    .await;
    assert_ok("duplicate category", status, &body);
    assert!(body.contains("分类名称已经存在"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!(
            "name={}&slug=source-cat&description={}&parent=0&do=insert",
            enc("源分类"),
            enc("源描述")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "create source");
    assert!(location_of(&headers).contains("notice="), "{}", location_of(&headers));
    let source: i64 = sqlx::query_scalar(r#"SELECT mid FROM "typecho_metas" WHERE slug = 'source-cat'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let (status, body, _) = authed(&app, &db, "GET", &format!("/admin/category.php?mid={source}"), &cookie, "").await;
    assert_ok("category edit", status, &body);
    assert!(body.contains("分类名称"), "{body}");
    assert!(body.contains("分类缩略名"), "{body}");
    assert!(body.contains("父级分类"), "{body}");
    assert!(body.contains("分类描述"), "{body}");
    assert!(body.contains("编辑分类"), "{body}");
    assert!(body.contains("源分类"), "{body}");
    assert!(body.contains("源描述"), "{body}");
    assert!(body.contains("不选择"), "{body}");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!(
            "name={}&slug=source-cat&description={}&parent=0&do=update&mid={source}",
            enc("源分类改"),
            enc("新描述")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "update category");
    let description: String = sqlx::query_scalar(r#"SELECT description FROM "typecho_metas" WHERE mid = ?"#)
        .bind(source)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(description, "新描述");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!("name={}&slug=child-cat&parent={source}&do=insert", enc("子分类")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "create child");
    let child: i64 = sqlx::query_scalar(r#"SELECT mid FROM "typecho_metas" WHERE slug = 'child-cat'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let child_parent: i64 = sqlx::query_scalar(r#"SELECT parent FROM "typecho_metas" WHERE mid = ?"#)
        .bind(child)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(child_parent, source);

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-categories.php", &cookie, "").await;
    assert_ok("children link", status, &body);
    assert!(body.contains("一个分类"), "{body}");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!("name={}&slug=&parent=0&do=insert", enc("分类甲")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "chinese slug");
    let zh_slug: String = sqlx::query_scalar(r#"SELECT slug FROM "typecho_metas" WHERE name = ?"#)
        .bind("分类甲")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(zh_slug, "分类甲");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit",
        &cookie,
        &format!("name={}&slug=target-cat&parent=0&do=insert", enc("目标分类")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "create target");
    let target: i64 = sqlx::query_scalar(r#"SELECT mid FROM "typecho_metas" WHERE slug = 'target-cat'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query(r#"INSERT INTO "typecho_relationships" ("cid","mid") VALUES (2, ?)"#)
        .bind(source)
        .execute(&db.pool)
        .await
        .unwrap();

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit?do=merge",
        &cookie,
        &format!("mid%5B%5D={source}&merge={target}"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "merge");
    assert!(location_of(&headers).contains(&enc("分类已经合并")) || location_of(&headers).contains("notice="));
    let source_left: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_metas" WHERE mid = ?"#)
        .bind(source)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(source_left, 0);
    let moved: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_relationships" WHERE cid = 2 AND mid = ?"#)
        .bind(target)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(moved, 1);
    let old_rel: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_relationships" WHERE mid = ?"#)
        .bind(source)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(old_rel, 0);
    let child_parent: i64 = sqlx::query_scalar(r#"SELECT parent FROM "typecho_metas" WHERE mid = ?"#)
        .bind(child)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(child_parent, target);
    let still_notes: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_relationships" WHERE cid = 2 AND mid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(still_notes, 1);

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/metas-category-edit?do=delete",
        &cookie,
        &format!("mid%5B%5D={target}"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "delete merged target");
    let child_parent: i64 = sqlx::query_scalar(r#"SELECT parent FROM "typecho_metas" WHERE mid = ?"#)
        .bind(child)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(child_parent, 0, "children move to the deleted category's parent");

    let (status, _, _) = authed(&app, &db, "GET", "/action/metas-category-edit?do=default&mid=1", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "default category");
    let default_mid: String = sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'defaultCategory'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(default_mid, "1");

    sqlx::query(r#"UPDATE "typecho_metas" SET count = 99 WHERE mid = 1"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, _, _) = authed(&app, &db, "POST", "/action/metas-category-edit?do=refresh", &cookie, "mid%5B%5D=1").await;
    assert_eq!(status, StatusCode::FOUND, "refresh");
    let count: i64 = sqlx::query_scalar(r#"SELECT count FROM "typecho_metas" WHERE mid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 2);

    let (ctype, bytes) = multipart(&[("parent", "1"), ("ajax", "1")], "hello.txt", b"hello-attachment");
    let (status, body, _) = authed_bytes(&app, &db, "POST", "/action/upload", &cookie, &ctype, bytes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("upload json");
    let file_cid = payload[1]["cid"].as_i64().expect("cid");
    assert_eq!(payload[1]["title"], "hello.txt");
    assert!(payload[0].as_str().unwrap_or("").contains("usr/uploads/"));
    let on_disk = std::fs::read_dir(&uploads).unwrap().count() > 0
        || walk_has_file(&uploads, "hello.txt");
    assert!(on_disk, "uploaded bytes stored");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-medias.php", &cookie, "").await;
    assert_ok("media list", status, &body);
    assert!(body.contains("所属文章"), "{body}");
    assert!(body.contains("清理未归档文件"), "{body}");
    assert!(body.contains(&format!("/admin/media.php?cid={file_cid}")), "{body}");
    assert!(body.contains("你好世界"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", &format!("/admin/media.php?cid={file_cid}"), &cookie, "").await;
    assert_ok("media edit", status, &body);
    assert!(body.contains("标题"), "{body}");
    assert!(body.contains("缩略名"), "{body}");
    assert!(body.contains("描述"), "{body}");
    assert!(body.contains("hello.txt"), "{body}");
    assert!(body.contains("提交修改"), "{body}");
    assert!(body.contains("选择文件上传"), "{body}");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        &format!("/action/contents-attachment-edit?cid={file_cid}"),
        &cookie,
        &format!("name={}&slug=hello-file&description={}&do=update&cid={file_cid}", enc("说明文件"), enc("文件说明")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "update attachment");
    let title: String = sqlx::query_scalar(r#"SELECT title FROM "typecho_contents" WHERE cid = ?"#)
        .bind(file_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(title, "说明文件");
    let text: String = sqlx::query_scalar(r#"SELECT text FROM "typecho_contents" WHERE cid = ?"#)
        .bind(file_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(text.contains("文件说明"), "{text}");
    let parent: i64 = sqlx::query_scalar(r#"SELECT parent FROM "typecho_contents" WHERE cid = ?"#)
        .bind(file_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(parent, 1);

    let (status, body, _) = authed(&app, &db, "GET", "/admin/write-post.php?cid=1", &cookie, "").await;
    assert_ok("write attachments", status, &body);
    assert!(body.contains("id=\"tab-files\""), "{body}");
    assert!(body.contains("id=\"file-list\""), "{body}");
    assert!(body.contains("点击插入文件"), "{body}");
    assert!(body.contains("说明文件"), "{body}");
    assert!(body.contains("选择文件上传"), "{body}");

    let (ctype, bytes) = multipart(&[("parent", "0"), ("ajax", "1")], "loose.txt", b"loose");
    let (status, body, _) = authed_bytes(&app, &db, "POST", "/action/upload", &cookie, &ctype, bytes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let loose: serde_json::Value = serde_json::from_str(&body).unwrap();
    let loose_cid = loose[1]["cid"].as_i64().unwrap();
    let (status, body, _) = authed(&app, &db, "GET", "/admin/write-post.php", &cookie, "").await;
    assert_ok("new post unattached", status, &body);
    assert!(body.contains("loose.txt"), "{body}");

    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/action/contents-post-edit?cid=2",
        &cookie,
        &format!(
            "title={}&slug=second-post&text={}&do=publish&created=1700001000&allowComment=1&allowPing=1&allowFeed=1&attachment%5B%5D={loose_cid}",
            enc("第二篇"),
            enc("第二篇正文")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "attach on save");
    let parent: i64 = sqlx::query_scalar(r#"SELECT parent FROM "typecho_contents" WHERE cid = ?"#)
        .bind(loose_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(parent, 2);

    let (ctype, bytes) = multipart(&[("parent", "0"), ("ajax", "1")], "gone.txt", b"gone");
    let (status, body, _) = authed_bytes(&app, &db, "POST", "/action/upload", &cookie, &ctype, bytes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let gone: serde_json::Value = serde_json::from_str(&body).unwrap();
    let gone_cid = gone[1]["cid"].as_i64().unwrap();
    let (status, _, _) = authed(&app, &db, "POST", "/action/contents-attachment-edit?do=clear", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "clear");
    let left: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_contents" WHERE cid = ?"#)
        .bind(gone_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let kept: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_contents" WHERE cid = ?"#)
        .bind(file_cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn urls_follow_typecho_rewrite_and_admin_paths() {
    let (app, db, _uploads) = boot().await;
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();

    let (status, _, headers) = get(&app, "/admin/index.php").await;
    assert_eq!(status, StatusCode::FOUND, "guest admin");
    let loc = location_of(&headers);
    assert!(loc.contains("/admin/login.php?referer="), "{loc}");
    let (status, body, _) = get(&app, "/admin/login.php").await;
    assert_ok("login page", status, &body);
    assert!(body.contains("/index.php/action/login?_="), "{body}");

    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    assert_eq!(location_of(&headers), "/admin/index.php");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    for path in [
        "/admin/index.php",
        "/admin/write-post.php",
        "/admin/manage-posts.php",
        "/admin/write-page.php",
        "/admin/manage-pages.php",
        "/admin/manage-comments.php",
        "/admin/manage-categories.php",
        "/admin/category.php",
        "/admin/manage-tags.php",
        "/admin/manage-medias.php",
        "/admin/manage-users.php",
        "/admin/user.php",
        "/admin/profile.php",
        "/admin/options-general.php",
        "/admin/options-discussion.php",
        "/admin/options-reading.php",
        "/admin/options-permalink.php",
        "/admin/plugins.php",
        "/admin/themes.php",
        "/admin/options-theme.php",
        "/admin/theme-editor.php",
        "/admin/backup.php",
        "/admin/upgrade.php",
    ] {
        let (status, body, _) = authed(&app, &db, "GET", path, &cookie, "").await;
        assert_ok(path, status, &body);
    }
    let (status, body, _) = authed(&app, &db, "GET", "/admin/register.php", &cookie, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (_status, body, _) = authed(&app, &db, "GET", "/admin/options-permalink.php", &cookie, "").await;
    assert!(body.contains("/index.php/action/options-permalink?_="), "{body}");
    assert!(body.contains("默认风格"), "{body}");
    assert!(body.contains("wordpress风格"), "{body}");
    assert!(body.contains("按日期归档"), "{body}");
    assert!(body.contains("按分类归档"), "{body}");
    assert!(body.contains("个性化定义"), "{body}");
    assert!(body.contains("插件"), "{body}");
    assert!(body.contains("外观"), "{body}");
    assert!(body.contains("备份"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/themes.php", &cookie, "").await;
    assert_ok("themes copy", status, &body);
    assert!(body.contains("THEME_DIR"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/index.php", &cookie, "").await;
    assert_ok("dashboard version", status, &body);
    assert!(body.contains("1.2.1-compat"), "{body}");
    assert!(body.contains("不拉取外部日志"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/upgrade.php", &cookie, "").await;
    assert_ok("upgrade", status, &body);
    assert!(body.contains("手动升级"), "{body}");
    assert!(body.contains("替换这个程序的二进制"), "{body}");

    let legacy = [
        ("/admin/posts", "/admin/manage-posts.php"),
        ("/admin/posts/new", "/admin/write-post.php"),
        ("/admin/posts/2", "/admin/write-post.php?cid=2"),
        ("/admin/pages/6", "/admin/write-page.php?cid=6"),
        ("/admin/categories", "/admin/manage-categories.php"),
        ("/admin/categories/new", "/admin/category.php"),
        ("/admin/categories/1", "/admin/category.php?mid=1"),
        ("/admin/media", "/admin/manage-medias.php"),
        ("/admin/media/3", "/admin/media.php?cid=3"),
        ("/admin/tags", "/admin/manage-tags.php"),
        ("/admin/users", "/admin/manage-users.php"),
        ("/admin/profile", "/admin/profile.php"),
        ("/admin/settings/permalink", "/admin/options-permalink.php"),
        ("/admin/logout", "/index.php/action/logout"),
        ("/admin/login", "/admin/login.php"),
    ];
    for (from, to) in legacy {
        let (status, _, headers) = get(&app, from).await;
        assert_eq!(status, StatusCode::MOVED_PERMANENTLY, "{from}");
        assert_eq!(location_of(&headers), to, "{from}");
    }

    let (status, body, headers) = get(&app, "/archives/1/").await;
    assert_ok("rewrite on post", status, &body);
    assert_eq!(
        headers.get("x-pingback").and_then(|v| v.to_str().ok()),
        Some("http://test.local/action/xmlrpc")
    );
    assert!(body.contains("archives&#x2f;1&#x2f;comment"), "{body}");
    assert!(body.contains("href=\"/feed/\""), "{body}");
    assert!(!body.contains("/index.php/"), "{body}");

    sqlx::query(r#"UPDATE "typecho_options" SET value = '0' WHERE name = 'rewrite'"#)
        .execute(&db.pool)
        .await
        .unwrap();

    let (status, body, headers) = get(&app, "/").await;
    assert_ok("rewrite off home", status, &body);
    assert_eq!(
        headers.get("x-pingback").and_then(|v| v.to_str().ok()),
        Some("http://test.local/index.php/action/xmlrpc")
    );
    assert!(body.contains("/index.php/page/2/"), "{body}");
    assert!(body.contains("/index.php/archives/"), "{body}");
    let (status, _, _) = get(&app, "/index.php").await;
    assert_eq!(status, StatusCode::OK, "index.php entry stays on the home page");
    let (status, _, _) = get(&app, "/index.php/").await;
    assert_eq!(status, StatusCode::OK);

    let off = [
        ("/archives/1/", "/index.php/archives/1/"),
        ("/archives/1", "/index.php/archives/1/"),
        ("/index.php/archives/1", "/index.php/archives/1/"),
        ("/page/2/", "/index.php/page/2/"),
        ("/category/notes/", "/index.php/category/notes/"),
        ("/tag/rust/", "/index.php/tag/rust/"),
        ("/author/1/", "/index.php/author/1/"),
        ("/about.html", "/index.php/about.html"),
        ("/feed/", "/index.php/feed/"),
        ("/feed", "/index.php/feed/"),
        ("/feed/rss/", "/index.php/feed/rss/"),
        ("/feed/atom/", "/index.php/feed/atom/"),
        ("/feed/archives/1/", "/index.php/feed/archives/1/"),
        ("/feed/category/notes/", "/index.php/feed/category/notes/"),
        ("/2023/11/14/", "/index.php/2023/11/14/"),
    ];
    for (from, to) in off {
        let (status, _, headers) = get(&app, from).await;
        assert_eq!(status, StatusCode::MOVED_PERMANENTLY, "{from}");
        assert_eq!(location_of(&headers), to, "{from}");
        let (status, body, _) = get(&app, to).await;
        assert_eq!(status, StatusCode::OK, "{to}\n{body}");
    }
    let (_status, body, _) = get(&app, "/index.php/archives/1/").await;
    assert!(body.contains("index.php&#x2f;archives&#x2f;1&#x2f;comment"), "{body}");
    assert!(body.contains("/index.php/feed/"), "{body}");
    let (status, _, headers) = get(&app, "/?s=%E7%A7%98%E5%AF%86").await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        location_of(&headers).contains("/index.php/search/"),
        "{}",
        location_of(&headers)
    );

    let (status, _, headers) = authed(&app, &db, "GET", "/index.php/action/logout", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "logout");
    assert_eq!(location_of(&headers), "/admin/login.php");

    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "relogin");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/options-permalink",
        &cookie,
        "rewrite=1&postPattern=/archives/[cid:digital]/&pagePattern=/{slug}.html&categoryPattern=/category/{slug}/",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "rewrite probe");
    let loc = location_of(&headers);
    assert!(loc.contains("rewrite_check=1"), "{loc}");
    let (status, body, _) = authed(&app, &db, "GET", &loc, &cookie, "").await;
    assert_ok("rewrite checkbox", status, &body);
    assert!(body.contains("重写功能检测失败, 请检查你的服务器设置"), "{body}");
    assert!(body.contains("如果你仍然想启用此功能, 请勾选这里"), "{body}");
    let rewrite: String = sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'rewrite'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rewrite, "0");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/options-permalink",
        &cookie,
        "rewrite=1&enableRewriteAnyway=1&postPattern=/archives/[slug].html&pagePattern=/{slug}.html&categoryPattern=/category/{directory}/",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "save wordpress style");
    assert!(location_of(&headers).contains("notice="), "{}", location_of(&headers));
    let (status, body, _) = get(&app, "/archives/hello-world.html").await;
    assert_ok("wordpress permalink", status, &body);
    assert!(body.contains("/archives/hello-world.html"), "{body}");
    let (status, _, _) = get(&app, "/archives/1/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/options-permalink",
        &cookie,
        "rewrite=1&postPattern=custom&customPattern=/{year}/{month}/{day}/{slug}.html&pagePattern=/{cid}.html&categoryPattern=/category/{mid}/",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "custom date");
    let (status, body, _) = get(&app, "/2023/11/14/hello-world.html").await;
    assert_ok("date permalink", status, &body);
    assert!(body.contains("/2023/11/14/hello-world.html"), "{body}");
    let (status, body, _) = get(&app, "/6.html").await;
    assert_ok("page cid", status, &body);
    assert!(body.contains("关于本站"), "{body}");
    let (status, body, _) = get(&app, "/category/1/").await;
    assert_ok("category mid", status, &body);
    assert!(body.contains("随笔"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/options-permalink",
        &cookie,
        "rewrite=1&postPattern=custom&customPattern=/{slug}&pagePattern=/{slug}.html&categoryPattern=/category/{slug}/",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "conflict");
    assert!(location_of(&headers).contains(&enc("自定义链接与现有规则存在冲突")), "{}", location_of(&headers));

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/options-permalink",
        &cookie,
        "rewrite=1&postPattern=/archives/[cid:digital]/&pagePattern=/static/&categoryPattern=/category/{slug}/",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "bad page pattern");
    assert!(location_of(&headers).contains(&enc("独立页面路径中没有包含")), "{}", location_of(&headers));

    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/action/ajax?do=remoteCallback")
            .header("x-requested-with", "XMLHttpRequest")
            .body(Body::from("do=remoteCallback"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "OK");
    let (status, body, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/index.php/action/xmlrpc")
            .header("content-type", "text/xml")
            .body(Body::from("nope"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("fault"), "{body}");
}

#[tokio::test]
async fn appearance_options_and_backup_round_trip() {
    let (app, db, uploads) = boot().await;
    let themes = uploads.with_extension("themes");
    let backups = uploads.with_extension("backups");
    std::fs::create_dir_all(themes.join("plain")).expect("plain theme");
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(&stored)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(r#"UPDATE "typecho_options" SET value = 'file:missing.html' WHERE name = 'frontPage'"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/themes.php", &cookie, "").await;
    assert_ok("themes", status, &body);
    assert!(body.contains("可以使用的外观"), "{body}");
    assert!(body.contains("默认外观"), "{body}");
    assert!(body.contains("plain"), "{body}");
    assert!(body.contains("THEME_DIR"), "{body}");
    assert!(body.contains("/action/themes-edit?change=plain"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/options-theme.php", &cookie, "").await;
    assert_ok("options theme", status, &body);
    assert!(body.contains("站点 LOGO 地址"), "{body}");
    assert!(body.contains("侧边栏显示"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/theme-editor.php", &cookie, "").await;
    assert_ok("theme editor", status, &body);
    assert!(body.contains("不编辑 PHP 外观"), "{body}");
    assert!(body.contains("functions.php"), "{body}");
    assert!(!body.contains("保存文件"), "{body}");
    let templates = uploads.with_extension("templates");
    std::fs::write(templates.join("single.html"), "<p>旧模板</p>").expect("template");
    let (status, body, _) = authed(
        &app,
        &db,
        "GET",
        "/admin/theme-editor.php?file=single.html",
        &cookie,
        "",
    )
    .await;
    assert_ok("theme editor file", status, &body);
    assert!(body.contains("保存文件"), "{body}");
    assert!(body.contains("编辑文件 single.html"), "{body}");
    assert!(body.contains("&lt;p&gt;旧模板&lt;/p&gt;"), "{body}");
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/themes-edit",
        &cookie,
        &format!("edit=single.html&theme=default&content={}", enc("<p>新模板</p>")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "save template");
    assert!(
        location_of(&headers).contains(&enc("文件 single.html 的更改已经保存")),
        "{}",
        location_of(&headers)
    );
    assert_eq!(
        std::fs::read_to_string(templates.join("single.html")).unwrap(),
        "<p>新模板</p>"
    );
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/themes-edit",
        &cookie,
        "edit=functions.php&content=nope",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "reject php");
    assert!(
        location_of(&headers).contains(&enc("您编辑的文件不存在")),
        "{}",
        location_of(&headers)
    );
    std::fs::write(templates.join("list.html"), "locked").expect("list");
    let mut perms = std::fs::metadata(templates.join("list.html"))
        .unwrap()
        .permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(templates.join("list.html"), perms).unwrap();
    let (status, body, _) = authed(
        &app,
        &db,
        "GET",
        "/admin/theme-editor.php?file=list.html",
        &cookie,
        "",
    )
    .await;
    assert_ok("readonly template", status, &body);
    assert!(body.contains("此文件无法写入"), "{body}");
    assert!(!body.contains("保存文件"), "{body}");
    let mut perms = std::fs::metadata(templates.join("list.html"))
        .unwrap()
        .permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(templates.join("list.html"), perms).unwrap();
    let (status, body, _) = authed(&app, &db, "GET", "/admin/plugins.php", &cookie, "").await;
    assert_ok("plugins", status, &body);
    assert!(body.contains("HelloWorld"), "{body}");
    assert!(body.contains("禁用的插件"), "{body}");
    assert!(body.contains("编译进同一程序"), "{body}");
    assert!(body.contains("/action/plugins-edit?activate=HelloWorld"), "{body}");

    let (status, _, headers) = authed(&app, &db, "GET", "/action/themes-edit?change=plain", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "enable plain");
    assert!(location_of(&headers).contains("notice="), "{}", location_of(&headers));
    let theme: String = sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'theme'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(theme, "plain");
    let old: Option<String> =
        sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'theme:default'"#)
            .fetch_optional(&db.pool)
            .await
            .unwrap();
    assert!(old.is_none(), "old theme settings removed");
    let front: String = sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'frontPage'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(front, "recent");
    let (status, _, headers) = authed(&app, &db, "GET", "/admin/options-theme.php", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "theme without config");
    assert!(
        location_of(&headers).contains(&enc("该外观没有设置项")),
        "{}",
        location_of(&headers)
    );
    let (status, body, _) = authed(&app, &db, "GET", "/admin/themes.php", &cookie, "").await;
    assert_ok("themes after plain", status, &body);
    assert!(!body.contains("设置外观"), "{body}");
    assert!(!body.contains("外观配置功能不存在"), "{body}");

    let (status, _, _) = authed(&app, &db, "GET", "/action/themes-edit?change=default", &cookie, "").await;
    assert_eq!(status, StatusCode::FOUND, "enable default");
    let saved: String =
        sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'theme:default'"#)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(saved.contains("ShowRecentPosts"), "{saved}");
    let form = format!(
        "logoUrl={}&sidebarBlock[]=ShowCategory",
        enc("https://example.com/logo.png")
    );
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/themes-edit?config",
        &cookie,
        &form,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "save theme config");
    assert!(location_of(&headers).contains("/admin/options-theme.php"), "{}", location_of(&headers));
    let (status, body, _) = get(&app, "/").await;
    assert_ok("home", status, &body);
    assert!(body.contains("id=\"logo\""), "{body}");
    assert!(body.contains("example.com"), "{body}");
    assert!(body.contains("分类"), "{body}");
    assert!(!body.contains("最新文章"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/backup.php", &cookie, "").await;
    assert_ok("backup page", status, &body);
    assert!(body.contains("不会涉及任何"), "{body}");
    assert!(body.contains("/index.php/action/backup?_="), "{body}");
    assert!(body.contains("恢复操作将清除所有现有数据, 是否继续?"), "{body}");
    assert!(body.contains("同时在服务器留一份"), "{body}");
    assert!(body.contains("导出默认只下载"), "{body}");
    let (status, body, _headers) = authed(&app, &db, "POST", "/index.php/action/backup", &cookie, "do=export").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        std::fs::read_dir(&backups)
            .expect("backup dir")
            .flatten()
            .all(|entry| {
                !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".dat"))
            }),
        "export without retain leaves no server copy"
    );
    let (status, body, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/backup",
        &cookie,
        "do=export&retain=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/octet-stream")
    );
    let disposition = headers
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(disposition.contains(".dat"), "{disposition}");
    let saved_file = std::fs::read_dir(&backups)
        .expect("backup dir")
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".dat"))
        })
        .expect("stored backup");
    let file_name = saved_file.file_name().to_string_lossy().into_owned();
    let bytes = std::fs::read(saved_file.path()).expect("read backup");
    assert!(bytes.starts_with(b"%TYPECHO_BACKUP_0001%"), "header");
    assert!(bytes.ends_with(b"%TYPECHO_BACKUP_0001%"), "footer");
    let (status, page, _) = authed(&app, &db, "GET", "/admin/backup.php", &cookie, "").await;
    assert_ok("backup list", status, &page);
    assert!(page.contains("do=download"), "{page}");
    assert!(page.contains("_="), "{page}");
    let (status, _, headers) = post(
        &app,
        "/index.php/action/backup",
        &format!("do=delete&file={}", enc(&file_name)),
        Some("http://test.local/admin/backup.php"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "unsigned delete");
    assert!(
        !location_of(&headers).contains(&enc("备份文件已经删除")),
        "{}",
        location_of(&headers)
    );
    assert!(backups.join(&file_name).exists(), "unsigned delete keeps the file");

    let (content_type, bad) = multipart(&[], "bad.dat", b"not-a-backup");
    let (status, _, headers) = authed_bytes(
        &app,
        &db,
        "POST",
        "/index.php/action/backup?do=import",
        &cookie,
        &content_type,
        bad,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "bad backup");
    assert!(location_of(&headers).contains("error="), "{}", location_of(&headers));
    let title: String = sqlx::query_scalar(r#"SELECT title FROM "typecho_contents" WHERE cid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(title, "你好世界");

    sqlx::query(r#"UPDATE "typecho_contents" SET title = '被改掉' WHERE cid = 1"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/backup",
        &cookie,
        &format!("do=import&file={}", enc(&file_name)),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "server import");
    assert!(location_of(&headers).contains("notice="), "{}", location_of(&headers));
    let title: String = sqlx::query_scalar(r#"SELECT title FROM "typecho_contents" WHERE cid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(title, "你好世界");
    let logo: String =
        sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'theme:default'"#)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(logo.contains("https://example.com/logo.png"), "settings stay out of the backup: {logo}");

    sqlx::query(r#"UPDATE "typecho_contents" SET title = '再次改掉' WHERE cid = 1"#)
        .execute(&db.pool)
        .await
        .unwrap();
    let cookie = session_cookie(&db).await;
    let (content_type, upload) = multipart(&[], "site.dat", &bytes);
    let (status, _, _) = authed_bytes(
        &app,
        &db,
        "POST",
        "/index.php/action/backup?do=import",
        &cookie,
        &content_type,
        upload,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "upload import");
    let title: String = sqlx::query_scalar(r#"SELECT title FROM "typecho_contents" WHERE cid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(title, "你好世界");

    let cookie = session_cookie(&db).await;
    let (status, _, _) = authed(
        &app,
        &db,
        "POST",
        "/index.php/action/backup",
        &cookie,
        &format!("do=delete&file={}", enc(&file_name)),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "delete backup");
    assert!(!backups.join(&file_name).exists(), "file removed");
}

#[tokio::test]
async fn builtin_plugin_activate_config_and_hooks() {
    let (app, db, _uploads) = boot().await;
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(&stored)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, home, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert!(!home.contains("message success"), "{home}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/plugins.php", &cookie, "").await;
    assert_ok("plugins", status, &body);
    assert!(body.contains("禁用的插件"), "{body}");
    assert!(body.contains("/action/plugins-edit?activate=HelloWorld"), "{body}");
    assert!(body.contains("usr/plugins"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "GET",
        "/action/plugins-edit?activate=HelloWorld",
        &cookie,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "activate");
    assert!(location_of(&headers).contains(&enc("插件已经被启用")), "{}", location_of(&headers));
    let (status, body, _) = authed(
        &app,
        &db,
        "GET",
        &format!("/admin/plugins.php?notice={}", enc("插件已经被启用")),
        &cookie,
        "",
    )
    .await;
    assert_ok("activate notice", status, &body);
    assert!(body.contains("插件已经被启用"), "{body}");
    assert!(body.contains("启用的插件"), "{body}");
    let plugins_row: String = sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'plugins'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(plugins_row.contains("HelloWorld"), "{plugins_row}");
    assert!(
        plugins_row.contains(r"admin/menu.php:navBar"),
        "{plugins_row}"
    );
    assert!(
        plugins_row.contains(r"TypechoPlugin\HelloWorld\Plugin::render"),
        "{plugins_row}"
    );
    let config_row: String =
        sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'plugin:HelloWorld'"#)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(config_row.contains("Hello World"), "{config_row}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "GET",
        "/action/plugins-edit?activate=HelloWorld",
        &cookie,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "second activate");
    assert!(location_of(&headers).contains(&enc("无法启用插件")), "{}", location_of(&headers));

    let (status, admin, _) = authed(&app, &db, "GET", "/admin/index.php", &cookie, "").await;
    assert_ok("dashboard", status, &admin);
    assert!(admin.contains(r#"<span class="message success">Hello World</span>"#), "{admin}");
    let (status, home, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert!(
        !home.contains(r#"<span class="message success">Hello World</span>"#),
        "{home}"
    );

    let (status, body, _) = authed(
        &app,
        &db,
        "GET",
        "/admin/options-plugin.php?config=HelloWorld",
        &cookie,
        "",
    )
    .await;
    assert_ok("plugin config", status, &body);
    assert!(body.contains("说点什么"), "{body}");
    assert!(body.contains("设置插件 HelloWorld"), "{body}");
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/plugins-edit?config=HelloWorld",
        &cookie,
        &format!("word={}", enc("插件钩子可见")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "save config");
    assert!(location_of(&headers).contains(&enc("插件设置已经保存")), "{}", location_of(&headers));
    let (status, home, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert!(!home.contains("插件钩子可见"), "{home}");
    assert!(!home.contains(">Hello World<"), "{home}");
    let (status, admin, _) = authed(&app, &db, "GET", "/admin/plugins.php", &cookie, "").await;
    assert_ok("plugins after save", status, &admin);
    assert!(admin.contains("插件钩子可见"), "{admin}");
    assert!(admin.contains("启用的插件"), "{admin}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "GET",
        "/action/plugins-edit?deactivate=HelloWorld",
        &cookie,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "deactivate");
    assert!(location_of(&headers).contains(&enc("插件已经被禁用")), "{}", location_of(&headers));
    let gone: Option<String> =
        sqlx::query_scalar(r#"SELECT value FROM "typecho_options" WHERE name = 'plugin:HelloWorld'"#)
            .fetch_optional(&db.pool)
            .await
            .unwrap();
    assert!(gone.is_none(), "plugin config removed");
    let (status, home, _) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert!(!home.contains("插件钩子可见"), "{home}");
    let (status, admin, _) = authed(&app, &db, "GET", "/admin/index.php", &cookie, "").await;
    assert_ok("dashboard after disable", status, &admin);
    assert!(!admin.contains("插件钩子可见"), "{admin}");
    let (status, body, _) = authed(
        &app,
        &db,
        "GET",
        "/admin/options-plugin.php?config=HelloWorld",
        &cookie,
        "",
    )
    .await;
    assert_ok("config while off", status, &body);
    assert!(body.contains("无法配置插件"), "{body}");
}

#[tokio::test]
async fn user_page_adds_and_edits_one_user() {
    let (app, db, _uploads) = boot().await;
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(&stored)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-users.php", &cookie, "").await;
    assert_ok("user list", status, &body);
    assert!(body.contains("<h2>管理用户<a href=\"/admin/user.php\">新增</a></h2>"), "{body}");
    assert!(body.contains("href=\"/admin/user.php?uid=1\""), "{body}");
    assert!(!body.contains("编号留空表示新建"), "{body}");
    assert!(body.contains("size-10\">5<"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/user.php", &cookie, "").await;
    assert_ok("create user", status, &body);
    assert!(body.contains("<h2>新增用户</h2>"), "{body}");
    assert!(body.contains("增加用户"), "{body}");
    assert!(body.contains("用户密码 *"), "{body}");
    assert!(body.contains("value=\"insert\""), "{body}");
    assert!(!body.contains("disabled=\"disabled\""), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/user.php?uid=1", &cookie, "").await;
    assert_ok("edit user", status, &body);
    assert!(body.contains("<h2>编辑用户 admin</h2>"), "{body}");
    assert!(body.contains("编辑用户</button>"), "{body}");
    assert!(body.contains("disabled=\"disabled\""), "{body}");
    assert!(body.contains("value=\"admin\""), "{body}");
    assert!(body.contains("value=\"站长\""), "{body}");
    assert!(body.contains("value=\"update\""), "{body}");
    assert!(body.contains("value=\"1\""), "{body}");
    assert!(body.contains(">用户密码</label>"), "{body}");
    assert!(!body.contains("用户密码 *"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/user.php?uid=99", &cookie, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body.contains("用户不存在"), "{body}");

    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/users-edit",
        &cookie,
        "name=&mail=&password=&confirm=&do=insert",
    )
    .await;
    assert_ok("blank user", status, &body);
    assert!(body.contains("必须填写用户名称"), "{body}");
    assert!(body.contains("必须填写电子邮箱"), "{body}");
    assert!(body.contains("必须填写密码"), "{body}");

    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/users-edit",
        &cookie,
        "name=editor1&mail=not-mail&screenName=&password=short&confirm=short&group=editor&do=insert",
    )
    .await;
    assert_ok("weak user", status, &body);
    assert!(body.contains("电子邮箱格式错误"), "{body}");
    assert!(body.contains("为了保证账户安全, 请输入至少六位的密码"), "{body}");

    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-edit",
        &cookie,
        &format!(
            "name=editor1&mail=ed@example.com&screenName={}&password=longpass&confirm=longpass&url=https://ed.example&group=editor&do=insert",
            enc("编辑一")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "created");
    assert!(location_of(&headers).contains(&enc("用户 编辑一 已经被增加")), "{}", location_of(&headers));
    let created: (i64, String) = sqlx::query_as(r#"SELECT uid, password FROM "typecho_users" WHERE name = 'editor1'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(created.1.starts_with("$P$"), "{}", created.1);
    let (status, list, _) = authed(&app, &db, "GET", "/admin/manage-users.php", &cookie, "").await;
    assert_ok("list after create", status, &list);
    assert!(list.contains(&format!("href=\"/admin/user.php?uid={}\"", created.0)), "{list}");
    let (status, _) = login_as(&app, "editor1", "longpass").await;
    assert_eq!(status, StatusCode::FOUND, "new user login");

    let before: String = sqlx::query_scalar(r#"SELECT password FROM "typecho_users" WHERE uid = 1"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let (status, _, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/users-edit",
        &cookie,
        &format!(
            "name=hacker&mail=admin@example.com&screenName={}&url=https://example.com&password=&confirm=&group=administrator&do=update&uid=1",
            enc("新站长")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "updated");
    assert!(location_of(&headers).contains(&enc("用户 新站长 已经被更新")), "{}", location_of(&headers));
    let row: (String, String, String) = sqlx::query_as(
        r#"SELECT name, screenName, password FROM "typecho_users" WHERE uid = 1"#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "admin");
    assert_eq!(row.1, "新站长");
    assert_eq!(row.2, before);

    let (status, body, _) = authed(
        &app,
        &db,
        "POST",
        "/action/users-edit",
        &cookie,
        "mail=admin@example.com&screenName=站长&password=longpass&confirm=otherpass&group=administrator&do=update&uid=1",
    )
    .await;
    assert_ok("password mismatch", status, &body);
    assert!(body.contains("两次输入的密码不一致"), "{body}");
    assert!(body.contains("value=\"站长\""), "{body}");
}

#[tokio::test]
async fn typecho_drafts_zero_cid_sidebar_and_empty_tags() {
    let (app, db, _uploads) = boot().await;
    for (cid, title, slug, kind) in [
        (40_i64, "外来草稿", "typecho-draft", "post_draft"),
        (41, "外来页面草稿", "typecho-page-draft", "page_draft"),
        (42, "本地草稿", "local-draft", "post"),
    ] {
        let status_name = if cid == 42 { "draft" } else { "publish" };
        sqlx::query(
            r#"INSERT INTO "typecho_contents"
            ("cid","title","slug","created","modified","text","order","authorId","template","type","status","password","commentsNum","allowComment","allowPing","allowFeed","parent")
            VALUES (?, ?, ?, 1700002000, 1700002000, '草稿正文', 0, 1, NULL, ?, ?, '', 0, '1', '1', '1', 0)"#,
        )
        .bind(cid)
        .bind(title)
        .bind(slug)
        .bind(kind)
        .bind(status_name)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let stored = tiebidli::auth::md5_hex(b"adminpass");
    sqlx::query(r#"UPDATE "typecho_users" SET password = ? WHERE name = 'admin'"#)
        .bind(stored)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, headers) = login_as(&app, "admin", "adminpass").await;
    assert_eq!(status, StatusCode::FOUND, "login");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-posts.php", &cookie, "").await;
    assert_ok("available posts", status, &body);
    assert!(body.contains("第二篇"), "{body}");
    assert!(!body.contains("外来草稿"), "{body}");
    assert!(!body.contains("本地草稿"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-posts.php?status=draft", &cookie, "").await;
    assert_ok("draft posts", status, &body);
    assert!(body.contains("外来草稿"), "{body}");
    assert!(body.contains("本地草稿"), "{body}");
    assert!(body.contains("草稿"), "{body}");
    assert!(body.contains("/admin/write-post.php?cid=40"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/write-post.php?cid=40", &cookie, "").await;
    assert_ok("edit typecho draft", status, &body);
    assert!(body.contains("外来草稿"), "{body}");
    assert!(body.contains("当前正在编辑的是未发布的草稿"), "{body}");
    assert!(!body.contains("不能编辑这条内容"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-pages.php?status=draft", &cookie, "").await;
    assert_ok("draft pages", status, &body);
    assert!(body.contains("外来页面草稿"), "{body}");
    let (status, body, _) = authed(&app, &db, "GET", "/admin/manage-pages.php", &cookie, "").await;
    assert_ok("available pages", status, &body);
    assert!(!body.contains("外来页面草稿"), "{body}");

    let (status, body, _) = authed(&app, &db, "GET", "/admin/index.php", &cookie, "").await;
    assert_ok("dashboard", status, &body);
    assert!(!body.contains("外来草稿"), "{body}");

    let (status, form, _) = authed(&app, &db, "GET", "/admin/write-post.php", &cookie, "").await;
    assert_ok("create form", status, &form);
    assert!(!form.contains("name=\"cid\" value=\"0\""), "{form}");
    assert!(!form.contains("内容不存在"), "{form}");
    let (status, missing, _) = authed(&app, &db, "GET", "/admin/write-post.php?cid=0", &cookie, "").await;
    assert_ok("cid zero is create", status, &missing);
    assert!(missing.contains("撰写新文章"), "{missing}");
    assert!(!missing.contains("内容不存在"), "{missing}");

    let (status, body, headers) = authed(
        &app,
        &db,
        "POST",
        "/action/contents-post-edit",
        &cookie,
        &format!(
            "cid=0&title={}&slug=zero-cid&text=zero-body&do=publish&visibility=publish&allowComment=1&allowPing=1&allowFeed=1",
            enc("零号新建")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "{body}");
    assert!(!body.contains("内容不存在"), "{body}");
    assert!(location_of(&headers).contains("/admin/write-post.php?cid="), "{}", location_of(&headers));
    let saved: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "typecho_contents" WHERE title = '零号新建' AND type = 'post'"#)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(saved, 1);

    let (status, home, _) = get(&app, "/").await;
    assert_ok("home sidebar", status, &home);
    assert!(home.contains("最新文章"), "{home}");
    assert!(home.contains("密码保护的文章"), "{home}");
    assert!(home.contains("/archives/3/"), "{home}");
    assert!(!home.contains("密码文"), "{home}");

    let (status, bare, _) = get(&app, "/archives/2/").await;
    assert_ok("empty tags", status, &bare);
    assert!(bare.contains("第二篇"), "{bare}");
    assert!(!bare.contains("标签:"), "{bare}");
    assert!(!bare.contains("标签: none"), "{bare}");
    let (status, tagged, _) = get(&app, "/archives/1/").await;
    assert_ok("tagged post", status, &tagged);
    assert!(tagged.contains("标签:"), "{tagged}");
    assert!(tagged.contains("Rust"), "{tagged}");
}

fn enc(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn multipart(fields: &[(&str, &str)], filename: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----tiebidliBoundary";
    let mut body = Vec::new();
    for (key, value) in fields {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n").into_bytes());
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n")
            .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn walk_has_file(dir: &std::path::Path, name_part: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && walk_has_file(&path, name_part) {
            return true;
        }
        if path.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.contains(name_part)) {
            return true;
        }
    }
    false
}

async fn authed_bytes(
    app: &axum::Router,
    db: &Db,
    method: &str,
    path: &str,
    cookie: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let (path, referer) = sign_action(db, path).await;
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("content-type", content_type);
    if let Some(referer) = referer {
        b = b.header("referer", referer);
    }
    call(app, b.body(Body::from(bytes)).unwrap()).await
}

async fn session_cookie(db: &Db) -> String {
    let code = auth_code(db).await.unwrap_or_default();
    format!(
        "{}=1; {}={}",
        tiebidli::auth::cookie_name("http://test.local", "__typecho_uid"),
        tiebidli::auth::cookie_name("http://test.local", "__typecho_authCode"),
        urlencoding::encode(&tiebidli::auth::typecho_hash(&code, "tiebidli1"))
    )
}

async fn auth_code(db: &Db) -> Option<String> {
    let code: Option<String> = sqlx::query_scalar(r#"SELECT authCode FROM "typecho_users" WHERE uid = 1"#)
        .fetch_optional(&db.pool)
        .await
        .unwrap();
    code.filter(|s| !s.is_empty())
}

async fn sign_action(db: &Db, path: &str) -> (String, Option<&'static str>) {
    if !path.contains("/action/") {
        return (path.to_string(), None);
    }
    let referer = "http://test.local/admin/index.php";
    let code = auth_code(db).await;
    let token = tiebidli::auth::security_token("test-secret", code.as_deref().map(|c| (c, 1)), referer);
    let path = if path.contains('?') {
        format!("{path}&_={token}")
    } else {
        format!("{path}?_={token}")
    };
    (path, Some(referer))
}

async fn login_as(app: &axum::Router, name: &str, password: &str) -> (StatusCode, axum::http::HeaderMap) {
    let referer = "http://test.local/admin/login.php";
    let token = tiebidli::auth::security_token("test-secret", None, referer);
    let path = format!("/index.php/action/login?_={token}");
    let (status, _, headers) = post(
        app,
        &path,
        &format!("name={name}&password={password}"),
        Some(referer),
    )
    .await;
    (status, headers)
}

fn location_of(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("/admin/profile.php")
        .to_string()
}

async fn authed(
    app: &axum::Router,
    db: &Db,
    method: &str,
    path: &str,
    cookie: &str,
    form: &str,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let (path, referer) = sign_action(db, path).await;
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie);
    if let Some(referer) = referer {
        b = b.header("referer", referer);
    }
    if method == "POST" {
        b = b.header("content-type", "application/x-www-form-urlencoded");
    }
    let body = if method == "POST" {
        Body::from(form.to_string())
    } else {
        Body::empty()
    };
    call(app, b.body(body).unwrap()).await
}
