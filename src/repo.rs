use crate::db::{col_i64, col_str, is_yes, Db, Dialect};
use crate::model::{site_from_parts, Comment, FieldValue, Meta, Post, Site};
use std::collections::HashMap;

impl Db {
    pub async fn load_site(&self, now: i64) -> Result<Site, sqlx::Error> {
        let opts = self.load_options().await?;
        let categories = self.metas_by_type("category").await?;
        let tags = self.metas_by_type("tag").await?;
        let pages = self.published_pages(now).await?;
        Ok(site_from_parts(opts, categories, tags, pages))
    }

    async fn load_options(&self) -> Result<HashMap<String, String>, sqlx::Error> {
        let sql = format!(
            "SELECT name, value FROM {} WHERE {} = 0",
            self.table("options"),
            self.quote("user")
        );
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        let mut map = HashMap::new();
        for row in rows {
            map.insert(col_str(&row, "name"), col_str(&row, "value"));
        }
        Ok(map)
    }

    async fn metas_by_type(&self, kind: &str) -> Result<Vec<Meta>, sqlx::Error> {
        let order = self.order_col();
        let sql = format!(
            "SELECT mid, name, slug, type, description, count, {order} AS sort_order, parent \
             FROM {} WHERE type = ? ORDER BY sort_order ASC, mid ASC",
            self.table("metas")
        );
        let rows = sqlx::query(&sql).bind(kind).fetch_all(&self.pool).await?;
        rows.iter().map(map_meta).collect()
    }

    pub async fn meta_by_slug(&self, kind: &str, slug: &str) -> Result<Option<Meta>, sqlx::Error> {
        let order = self.order_col();
        let sql = format!(
            "SELECT mid, name, slug, type, description, count, {order} AS sort_order, parent \
             FROM {} WHERE type = ? AND slug = ? LIMIT 1",
            self.table("metas")
        );
        let row = sqlx::query(&sql)
            .bind(kind)
            .bind(slug)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(map_meta).transpose()
    }

    async fn published_pages(&self, now: i64) -> Result<Vec<Post>, sqlx::Error> {
        let order = self.order_col();
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = 'page' AND c.status = 'publish' AND c.created < ? \
             ORDER BY c.{order} ASC, c.cid ASC",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
            order = order,
        );
        let mut posts = self.fetch_posts(&sql, &[Bind::I64(now)]).await?;
        self.attach(&mut posts).await?;
        Ok(posts)
    }

    pub async fn list_posts(
        &self,
        filter: &ListFilter,
        page: i64,
        page_size: i64,
        now: i64,
        feed_only: bool,
    ) -> Result<(Vec<Post>, i64), sqlx::Error> {
        let (mut extra_sql, extra_binds) = self.filter_sql(filter);
        if feed_only {
            extra_sql
                .push_str(" AND c.allowFeed = '1' AND (c.password IS NULL OR c.password = '')");
        }
        let where_sql =
            format!("c.type = 'post' AND c.status = 'publish' AND c.created < ? {extra_sql}");
        let mut binds = vec![Bind::I64(now)];
        binds.extend(extra_binds);
        let count_sql = format!(
            "SELECT COUNT(*) AS n FROM {contents} c WHERE {where_sql}",
            contents = self.table("contents")
        );
        let total = self.scalar_i64(&count_sql, &binds).await?;
        let page = page.max(1);
        let offset = (page - 1) * page_size;
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE {where_sql} ORDER BY c.created DESC, c.cid DESC LIMIT ? OFFSET ?",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        binds.push(Bind::I64(page_size));
        binds.push(Bind::I64(offset));
        let mut posts = self.fetch_posts(&sql, &binds).await?;
        self.attach(&mut posts).await?;
        Ok((posts, total))
    }

    pub async fn content_by_id(&self, cid: i64) -> Result<Option<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.cid = ? LIMIT 1",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        self.one_post(&sql, &[Bind::I64(cid)]).await
    }

    pub async fn single_by_cid(
        &self,
        cid: i64,
        kind: &str,
        now: i64,
    ) -> Result<Option<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.cid = ? AND c.type = ? AND (c.status = 'publish' OR c.status = 'hidden') \
             AND c.created < ? LIMIT 1",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        self.one_post(&sql, &[Bind::I64(cid), Bind::text(kind), Bind::I64(now)])
            .await
    }

    pub async fn single_by_slug(
        &self,
        slug: &str,
        kind: &str,
        now: i64,
    ) -> Result<Option<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.slug = ? AND c.type = ? AND (c.status = 'publish' OR c.status = 'hidden') \
             AND c.created < ? LIMIT 1",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        self.one_post(&sql, &[Bind::text(slug), Bind::text(kind), Bind::I64(now)])
            .await
    }

    pub async fn neighbors(
        &self,
        post: &Post,
        now: i64,
    ) -> Result<(Option<Post>, Option<Post>), sqlx::Error> {
        let newer_sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = ? AND c.status = 'publish' AND c.created > ? AND c.created < ? \
             AND (c.password IS NULL OR c.password = '') \
             ORDER BY c.created ASC LIMIT 1",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        let older_sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = ? AND c.status = 'publish' AND c.created < ? \
             AND (c.password IS NULL OR c.password = '') \
             ORDER BY c.created DESC LIMIT 1",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        let newer = self
            .one_post(
                &newer_sql,
                &[
                    Bind::text(&post.kind),
                    Bind::I64(post.created),
                    Bind::I64(now),
                ],
            )
            .await?;
        let older = self
            .one_post(
                &older_sql,
                &[Bind::text(&post.kind), Bind::I64(post.created)],
            )
            .await?;
        let mut pair = Vec::new();
        if let Some(p) = newer.clone() {
            pair.push(p);
        }
        if let Some(p) = older.clone() {
            pair.push(p);
        }
        self.attach(&mut pair).await?;
        let newer = pair
            .iter()
            .find(|p| newer.as_ref().is_some_and(|n| n.cid == p.cid))
            .cloned();
        let older = pair
            .iter()
            .find(|p| older.as_ref().is_some_and(|n| n.cid == p.cid))
            .cloned();
        Ok((newer, older))
    }

    pub async fn comments_for(
        &self,
        cid: i64,
        author: &str,
        mail: &str,
        comment_only: bool,
    ) -> Result<Vec<Comment>, sqlx::Error> {
        let type_sql = if comment_only {
            " AND type = 'comment'"
        } else {
            ""
        };
        let sql = format!(
            "SELECT coid, cid, created, author, authorId, ownerId, mail, url, ip, text, type, status, parent \
             FROM {} WHERE cid = ? AND (status = 'approved' OR (status = 'waiting' AND author = ? AND mail = ?)) \
             {type_sql} ORDER BY coid ASC",
            self.table("comments")
        );
        let rows = sqlx::query(&sql)
            .bind(cid)
            .bind(author)
            .bind(mail)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_comment).collect())
    }

    pub async fn published_times(&self, now: i64) -> Result<Vec<i64>, sqlx::Error> {
        let sql = format!(
            "SELECT created FROM {} WHERE type = 'post' AND status = 'publish' AND created < ? ORDER BY created DESC",
            self.table("contents")
        );
        let rows = sqlx::query(&sql).bind(now).fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(col_i64(&row, "created")?);
        }
        Ok(out)
    }

    pub async fn recent_comments(
        &self,
        limit: i64,
        now: i64,
    ) -> Result<Vec<(Comment, Post)>, sqlx::Error> {
        let sql = format!(
            "SELECT cm.coid, cm.cid, cm.created, cm.author, cm.authorId, cm.ownerId, cm.mail, cm.url, cm.ip, \
             cm.text, cm.type, cm.status, cm.parent \
             FROM {comments} cm JOIN {contents} c ON cm.cid = c.cid \
             WHERE cm.status = 'approved' AND cm.type = 'comment' AND c.status = 'publish' \
             AND c.created < ? AND (c.password IS NULL OR c.password = '') \
             ORDER BY cm.created DESC LIMIT ?",
            comments = self.table("comments"),
            contents = self.table("contents"),
        );
        let rows = sqlx::query(&sql)
            .bind(now)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        let comments: Vec<Comment> = rows.iter().map(map_comment).collect();
        let mut out = Vec::new();
        for c in comments {
            if let Some(post) = self.single_by_cid(c.cid, "post", now + 1).await? {
                out.push((c, post));
            } else if let Some(post) = self.single_by_cid(c.cid, "page", now + 1).await? {
                out.push((c, post));
            }
        }
        Ok(out)
    }

    pub async fn hot_posts(&self, limit: i64, now: i64) -> Result<Vec<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = 'post' AND c.status = 'publish' AND c.created < ? \
             AND (c.password IS NULL OR c.password = '') \
             ORDER BY c.commentsNum DESC, c.cid DESC LIMIT ?",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        let mut posts = self
            .fetch_posts(&sql, &[Bind::I64(now), Bind::I64(limit)])
            .await?;
        self.attach(&mut posts).await?;
        Ok(posts)
    }

    /// 热门按浏览数（Access `views` 列；老库缺列时调用方回退到 `hot_posts`）。
    pub async fn hot_posts_by_views(&self, limit: i64, now: i64) -> Result<Vec<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = 'post' AND c.status = 'publish' AND c.created < ? \
             AND (c.password IS NULL OR c.password = '') \
             ORDER BY COALESCE(c.views, 0) DESC, c.cid DESC LIMIT ?",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
        );
        let mut posts = self
            .fetch_posts(&sql, &[Bind::I64(now), Bind::I64(limit)])
            .await?;
        self.attach(&mut posts).await?;
        Ok(posts)
    }

    pub async fn random_posts(&self, limit: i64, now: i64) -> Result<Vec<Post>, sqlx::Error> {
        let sql = format!(
            "SELECT {cols} FROM {contents} c LEFT JOIN {users} u ON c.authorId = u.uid \
             WHERE c.type = 'post' AND c.status = 'publish' AND c.created < ? \
             AND (c.password IS NULL OR c.password = '') \
             ORDER BY {rand} LIMIT ?",
            cols = self.content_cols(),
            contents = self.table("contents"),
            users = self.table("users"),
            rand = self.random_fn(),
        );
        let mut posts = self
            .fetch_posts(&sql, &[Bind::I64(now), Bind::I64(limit)])
            .await?;
        self.attach(&mut posts).await?;
        Ok(posts)
    }

    pub async fn stats(&self, now: i64) -> Result<Stats, sqlx::Error> {
        let posts = self
            .scalar_i64(
                &format!(
                    "SELECT COUNT(*) AS n FROM {} WHERE type = 'post' AND status = 'publish' AND created < ?",
                    self.table("contents")
                ),
                &[Bind::I64(now)],
            )
            .await?;
        let comments = self
            .scalar_i64(
                &format!(
                    "SELECT COUNT(*) AS n FROM {} WHERE status = 'approved' AND type = 'comment'",
                    self.table("comments")
                ),
                &[],
            )
            .await?;
        let first = self
            .scalar_i64(
                &format!(
                    "SELECT MIN(created) AS n FROM {} WHERE type = 'post' AND status = 'publish'",
                    self.table("contents")
                ),
                &[],
            )
            .await?;
        let last_post = self
            .scalar_i64(
                &format!(
                    "SELECT MAX(modified) AS n FROM {} WHERE status = 'publish'",
                    self.table("contents")
                ),
                &[],
            )
            .await?;
        let last_comment = self
            .scalar_i64(
                &format!(
                    "SELECT MAX(created) AS n FROM {} WHERE status = 'approved'",
                    self.table("comments")
                ),
                &[],
            )
            .await?;
        Ok(Stats {
            posts,
            comments,
            first_created: first,
            last_activity: last_post.max(last_comment),
        })
    }

    pub async fn latest_comment_at(&self, cid: i64, ip: &str) -> Result<i64, sqlx::Error> {
        self.scalar_i64(
            &format!(
                "SELECT MAX(created) AS n FROM {} WHERE cid = ? AND ip = ?",
                self.table("comments")
            ),
            &[Bind::I64(cid), Bind::text(ip)],
        )
        .await
    }

    pub async fn approved_by_author(&self, author: &str, mail: &str) -> Result<i64, sqlx::Error> {
        self.scalar_i64(
            &format!(
                "SELECT COUNT(*) AS n FROM {} WHERE author = ? AND mail = ? AND status = 'approved'",
                self.table("comments")
            ),
            &[Bind::text(author), Bind::text(mail)],
        )
        .await
    }

    pub async fn author_name(&self, uid: i64) -> Result<Option<String>, sqlx::Error> {
        let sql = format!(
            "SELECT screenName, name FROM {} WHERE uid = ? LIMIT 1",
            self.table("users")
        );
        let row = sqlx::query(&sql)
            .bind(uid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| {
            let screen = col_str(&r, "screenName");
            if screen.is_empty() {
                col_str(&r, "name")
            } else {
                screen
            }
        }))
    }

    pub async fn user_name_taken(&self, name: &str) -> Result<bool, sqlx::Error> {
        let n = self
            .scalar_i64(
                &format!(
                    "SELECT COUNT(*) AS n FROM {} WHERE screenName = ? OR name = ?",
                    self.table("users")
                ),
                &[Bind::text(name), Bind::text(name)],
            )
            .await?;
        Ok(n > 0)
    }

    pub async fn comment_parent(&self, coid: i64) -> Result<Option<(i64, i64)>, sqlx::Error> {
        let sql = format!(
            "SELECT coid, cid FROM {} WHERE coid = ? LIMIT 1",
            self.table("comments")
        );
        let row = sqlx::query(&sql)
            .bind(coid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| {
            (
                col_i64(&r, "coid").unwrap_or(0),
                col_i64(&r, "cid").unwrap_or(0),
            )
        }))
    }

    pub async fn insert_comment(&self, c: &NewComment) -> Result<i64, sqlx::Error> {
        let sql = format!(
            "INSERT INTO {} (cid, created, author, authorId, ownerId, mail, url, ip, agent, text, type, status, parent) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            self.table("comments")
        );
        let res = sqlx::query(&sql)
            .bind(c.cid)
            .bind(c.created)
            .bind(&c.author)
            .bind(c.author_id)
            .bind(c.owner_id)
            .bind(opt_str(&c.mail))
            .bind(opt_str(&c.url))
            .bind(opt_str(&c.ip))
            .bind(opt_str(&c.agent))
            .bind(&c.text)
            .bind(if c.kind.is_empty() { "comment" } else { c.kind.as_str() })
            .bind(&c.status)
            .bind(c.parent)
            .execute(&self.pool)
            .await?;
        let mut id = res.last_insert_id().unwrap_or(0);
        if id == 0 && self.dialect == Dialect::Sqlite {
            id = self
                .scalar_i64(
                    &format!("SELECT MAX(coid) AS n FROM {}", self.table("comments")),
                    &[],
                )
                .await?;
        }
        self.refresh_comment_count(c.cid).await?;
        Ok(id)
    }

    async fn refresh_comment_count(&self, cid: i64) -> Result<(), sqlx::Error> {
        let n = self
            .scalar_i64(
                &format!(
                    "SELECT COUNT(*) AS n FROM {} WHERE status = 'approved' AND cid = ?",
                    self.table("comments")
                ),
                &[Bind::I64(cid)],
            )
            .await?;
        let sql = format!(
            "UPDATE {} SET commentsNum = ? WHERE cid = ?",
            self.table("contents")
        );
        sqlx::query(&sql)
            .bind(n)
            .bind(cid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    fn filter_sql(&self, filter: &ListFilter) -> (String, Vec<Bind>) {
        match filter {
            ListFilter::Index => (String::new(), Vec::new()),
            ListFilter::Feed => (
                " AND c.allowFeed = '1' AND (c.password IS NULL OR c.password = '')".into(),
                Vec::new(),
            ),
            ListFilter::Category(mids) => {
                if mids.is_empty() {
                    return (" AND 1 = 0".into(), Vec::new());
                }
                let marks = vec!["?"; mids.len()].join(",");
                let sql = format!(
                    " AND c.cid IN (SELECT cid FROM {} WHERE mid IN ({marks}))",
                    self.table("relationships")
                );
                let binds = mids.iter().copied().map(Bind::I64).collect();
                (sql, binds)
            }
            ListFilter::Tag(mid) => (
                format!(
                    " AND c.cid IN (SELECT cid FROM {} WHERE mid = ?)",
                    self.table("relationships")
                ),
                vec![Bind::I64(*mid)],
            ),
            ListFilter::Author(uid) => (" AND c.authorId = ?".into(), vec![Bind::I64(*uid)]),
            ListFilter::Date { start, end } => (
                " AND c.created >= ? AND c.created < ?".into(),
                vec![Bind::I64(*start), Bind::I64(*end)],
            ),
            ListFilter::Search(q) => {
                let like = format!("%{}%", q.replace(' ', "%"));
                (
                    " AND (c.password IS NULL OR c.password = '') AND (c.title LIKE ? OR c.text LIKE ?)"
                        .into(),
                    vec![Bind::text(like.clone()), Bind::text(like)],
                )
            }
        }
    }

    fn content_cols(&self) -> String {
        let order = self.order_col();
        format!(
            "c.cid, c.title, c.slug, c.created, c.modified, c.text, c.{order} AS sort_order, \
             c.authorId, c.template, c.type, c.status, c.password, c.commentsNum, \
             c.allowComment, c.allowPing, c.allowFeed, c.parent, \
             u.screenName AS screen_name, u.name AS user_name, u.mail AS user_mail, u.url AS user_url"
        )
    }

    async fn one_post(&self, sql: &str, binds: &[Bind]) -> Result<Option<Post>, sqlx::Error> {
        let mut posts = self.fetch_posts(sql, binds).await?;
        self.attach(&mut posts).await?;
        Ok(posts.into_iter().next())
    }

    async fn fetch_posts(&self, sql: &str, binds: &[Bind]) -> Result<Vec<Post>, sqlx::Error> {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = bind_apply(q, b);
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(map_post).collect()
    }

    async fn scalar_i64(&self, sql: &str, binds: &[Bind]) -> Result<i64, sqlx::Error> {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = bind_apply(q, b);
        }
        let row = q.fetch_optional(&self.pool).await?;
        Ok(row
            .as_ref()
            .map(|r| col_i64(r, "n").unwrap_or(0))
            .unwrap_or(0))
    }

    async fn attach(&self, posts: &mut [Post]) -> Result<(), sqlx::Error> {
        if posts.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = posts.iter().map(|p| p.cid).collect();
        let marks = vec!["?"; ids.len()].join(",");
        let order = self.order_col();
        let meta_sql = format!(
            "SELECT r.cid AS rel_cid, m.mid, m.name, m.slug, m.type, m.description, m.count, \
             m.{order} AS sort_order, m.parent \
             FROM {rel} r JOIN {metas} m ON r.mid = m.mid WHERE r.cid IN ({marks})",
            rel = self.table("relationships"),
            metas = self.table("metas"),
        );
        let mut q = sqlx::query(&meta_sql);
        for id in &ids {
            q = q.bind(*id);
        }
        let meta_rows = q.fetch_all(&self.pool).await?;
        for row in &meta_rows {
            let cid = col_i64(row, "rel_cid")?;
            let meta = map_meta(row)?;
            if let Some(post) = posts.iter_mut().find(|p| p.cid == cid) {
                if meta.kind == "category" {
                    post.categories.push(meta);
                } else if meta.kind == "tag" {
                    post.tags.push(meta);
                }
            }
        }
        for post in posts.iter_mut() {
            post.categories
                .sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.mid.cmp(&b.mid)));
            post.tags.sort_by(|a, b| a.mid.cmp(&b.mid));
        }

        let field_sql = format!(
            "SELECT cid, name, type, str_value, int_value FROM {} WHERE cid IN ({marks})",
            self.table("fields")
        );
        let mut q = sqlx::query(&field_sql);
        for id in &ids {
            q = q.bind(*id);
        }
        let field_rows = q.fetch_all(&self.pool).await?;
        for row in &field_rows {
            let cid = col_i64(row, "cid")?;
            let name = col_str(row, "name");
            let mut str_value = col_str(row, "str_value");
            if col_str(row, "type") == "json" {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&str_value) {
                    if let Some(s) = v.as_str() {
                        str_value = s.to_string();
                    }
                }
            }
            let fv = FieldValue {
                str_value,
                int_value: col_i64(row, "int_value").unwrap_or(0),
            };
            if let Some(post) = posts.iter_mut().find(|p| p.cid == cid) {
                post.fields.insert(name, fv);
            }
        }
        // `contents.views` 列是 Access 插件补的；老库缺列时保持 0，不让列表失败。
        let views_sql = format!(
            "SELECT cid, views FROM {} WHERE cid IN ({marks})",
            self.table("contents")
        );
        let mut q = sqlx::query(&views_sql);
        for id in &ids {
            q = q.bind(*id);
        }
        if let Ok(views_rows) = q.fetch_all(&self.pool).await {
            for row in &views_rows {
                let cid = col_i64(row, "cid").unwrap_or(0);
                let views = col_i64(row, "views").unwrap_or(0).max(0);
                if let Some(post) = posts.iter_mut().find(|p| p.cid == cid) {
                    post.views = views;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum ListFilter {
    Index,
    Feed,
    Category(Vec<i64>),
    Tag(i64),
    Author(i64),
    Date { start: i64, end: i64 },
    Search(String),
}

pub struct Stats {
    pub posts: i64,
    pub comments: i64,
    pub first_created: i64,
    pub last_activity: i64,
}

pub struct NewComment {
    pub cid: i64,
    pub created: i64,
    pub author: String,
    pub author_id: i64,
    pub owner_id: i64,
    pub mail: String,
    pub url: String,
    pub ip: String,
    pub agent: String,
    pub text: String,
    pub status: String,
    pub kind: String,
    pub parent: i64,
}

enum Bind {
    I64(i64),
    Text(String),
}

impl Bind {
    fn text(s: impl AsRef<str>) -> Self {
        Bind::Text(s.as_ref().to_string())
    }
}

fn bind_apply<'a>(
    q: sqlx::query::Query<'a, sqlx::Any, sqlx::any::AnyArguments<'a>>,
    b: &'a Bind,
) -> sqlx::query::Query<'a, sqlx::Any, sqlx::any::AnyArguments<'a>> {
    match b {
        Bind::I64(n) => q.bind(*n),
        Bind::Text(s) => q.bind(s.as_str()),
    }
}

fn opt_str(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn map_post(row: &sqlx::any::AnyRow) -> Result<Post, sqlx::Error> {
    Ok(Post {
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
        allow_comment: is_yes(&col_str(row, "allowComment")),
        allow_ping: is_yes(&col_str(row, "allowPing")),
        allow_feed: is_yes(&col_str(row, "allowFeed")),
        parent: col_i64(row, "parent")?,
        author_screen: col_str(row, "screen_name"),
        author_name: col_str(row, "user_name"),
        author_mail: col_str(row, "user_mail"),
        author_url: col_str(row, "user_url"),
        categories: Vec::new(),
        tags: Vec::new(),
        fields: HashMap::new(),
        // 主查询不 SELECT views（老库缺列会整句失败），attach 里 best-effort 回填。
        views: 0,
    })
}

fn map_meta(row: &sqlx::any::AnyRow) -> Result<Meta, sqlx::Error> {
    Ok(Meta {
        mid: col_i64(row, "mid")?,
        name: col_str(row, "name"),
        slug: col_str(row, "slug"),
        kind: col_str(row, "type"),
        description: col_str(row, "description"),
        count: col_i64(row, "count")?,
        sort_order: col_i64(row, "sort_order")?,
        parent: col_i64(row, "parent")?,
    })
}

fn map_comment(row: &sqlx::any::AnyRow) -> Comment {
    Comment {
        coid: col_i64(row, "coid").unwrap_or(0),
        cid: col_i64(row, "cid").unwrap_or(0),
        created: col_i64(row, "created").unwrap_or(0),
        author: col_str(row, "author"),
        author_id: col_i64(row, "authorId").unwrap_or(0),
        owner_id: col_i64(row, "ownerId").unwrap_or(0),
        mail: col_str(row, "mail"),
        url: col_str(row, "url"),
        ip: col_str(row, "ip"),
        text: col_str(row, "text"),
        kind: col_str(row, "type"),
        status: col_str(row, "status"),
        parent: col_i64(row, "parent").unwrap_or(0),
    }
}

#[allow(dead_code)]
fn _row_marker() -> Option<sqlx::any::AnyRow> {
    None
}
