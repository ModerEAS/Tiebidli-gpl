//! MySQL 真库回归：`ensure_table` + `log_visit` 读写（默认 `#[ignore]`）。
//!
//! 运行：`TIEBIDLI_MYSQL_URL='mysql://root@127.0.0.1:3306/tiebidli_test' cargo test --test access_mysql -- --ignored`
//! 未设变量时自动跳过（打印原因后直接返回，不断言失败）。

use tiebidli::db::Db;

fn mysql_url() -> Option<String> {
    std::env::var("TIEBIDLI_MYSQL_URL").ok().filter(|s| !s.trim().is_empty())
}

#[tokio::test]
#[ignore]
async fn access_mysql_ensure_table_and_log_visit() {
    let Some(url) = mysql_url() else {
        eprintln!("skip: TIEBIDLI_MYSQL_URL 未设置");
        return;
    };
    let db = Db::connect(&url, "t_").await.expect("mysql connect");
    // 建表幂等：连建两次都不报错。
    tiebidli::access::ensure_table(&db).await.expect("ensure_table");
    tiebidli::access::ensure_table(&db).await.expect("ensure_table again");
    assert!(tiebidli::access::table_exists(&db).await);

    let before: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM `t_access_log`"#)
        .fetch_one(&db.pool)
        .await
        .expect("count");
    tiebidli::access::log_visit(
        &db,
        "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        "/feed/",
        "203.0.113.7",
        "https://ext.example/a",
        "https://ext.example/a",
        1_700_000_100,
        None,
        None,
    )
    .await
    .expect("log_visit");
    let after: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM `t_access_log`"#)
        .fetch_one(&db.pool)
        .await
        .expect("count");
    assert_eq!(after, before + 1);
    // `Any` 驱动解不了 MySQL `TINYINT(1)`（已知限制，生产查询只在 WHERE 用 robot，从不 SELECT 它）；
    // 测试用 `robot + 0` 转成驱动可解的整数再断言。
    let row = sqlx::query(r#"SELECT (robot + 0) AS robot, robot_id, browser_id, entrypoint FROM `t_access_log` ORDER BY id DESC LIMIT 1"#)
        .fetch_one(&db.pool)
        .await
        .expect("row");
    assert_eq!(tiebidli::db::col_i64(&row, "robot").unwrap(), 1);
    assert!(!tiebidli::db::col_str(&row, "robot_id").is_empty());
    assert_eq!(tiebidli::db::col_str(&row, "entrypoint"), "https://ext.example/a");
}
