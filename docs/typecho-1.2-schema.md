# Typecho 1.2.1 表结构

权威来源是 Typecho 标签 `v1.2.1`（`206880ba714d80bfb0638aeacb49c58b8fb1b327`）的安装脚本：

- `install/Mysql.sql` → `schema/typecho-1.2-mysql.sql`
- `install/SQLite.sql` → `schema/typecho-1.2-sqlite.sql`
- `install/Pgsql.sql` → `schema/typecho-1.2-pgsql.sql`（只作对照，运行时不连接）

MySQL 脚本里的 `%engine%`、`%charset%` 由 Typecho 安装器替换。表名里的 `typecho_` 对应环境变量 `TYPECHO_PREFIX`。

1.2 核心没有 `views`。已经升级过的库可能多出可空列，本程序不 `ALTER`，也不 `SELECT` 未知列。

## `typecho_contents`

| 列 | 1.2 MySQL | 说明 |
| --- | --- | --- |
| `cid` | int unsigned PK | |
| `title` / `slug` | varchar(150) | `slug` 唯一 |
| `created` / `modified` | int unsigned | 真实 Unix 时间 |
| `text` | longtext | SQLite 为 text |
| `order` | int unsigned | 保留字，查询时加引号 |
| `authorId` | int unsigned | |
| `template` | varchar(32) | |
| `type` | varchar(16) | `post` / `page` / `attachment` |
| `status` | varchar(16) | `publish` / `hidden` / `private` |
| `password` | varchar(32) | 明文。空表示不保护 |
| `commentsNum` | int unsigned | 已通过评论数，插入后重算 |
| `allowComment` / `allowPing` / `allowFeed` | char(1) | `1` 为允许 |
| `parent` | int unsigned | |

## `typecho_comments`

`coid` 主键。`author`、`mail` 为 varchar(150)，`url` varchar(255)，`agent` varchar(511)，`ip` varchar(64)，`text` text。`status` 使用 `approved`、`waiting`、`spam`。`parent` 为父评论 `coid`。

## `typecho_metas` 与 `typecho_relationships`

`metas`：`mid`、`name`、`slug`、`type`（`category` 或 `tag`，非空）、`description` varchar(150)、`count`、`order`、`parent`。

`relationships` 主键是 (`cid`, `mid`)。分类列表会带上全部子孙 `mid`。

## `typecho_fields`

主键 (`cid`, `name`)。`type` 默认 `str`。字符串读 `str_value`；`type=json` 且 `str_value` 是 JSON 字符串时取出里面的文本。公开页用到 `thumb`（`no` 表示不要图片）、`customSummary`、`navbar=hide`。

## `typecho_options`

主键 (`name`, `user`)。公开站只读 `user=0`。`value` 是 text。路由表和 `theme:{name}` 是 PHP 序列化字符串。

## `typecho_users`

`uid`、`name`、`password`、`mail`、`url`、`screenName`、`created`、`activated`、`logged`、`group`、`authCode`。`name` 和 `mail` 唯一。这一版不校验登录密码；评论昵称如果撞上 `name` 或 `screenName`，会要求先登录，因此匿名评论不能使用已注册的名字。
