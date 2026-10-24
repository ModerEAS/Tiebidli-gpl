# Access 对齐补齐结果（PR #3 之后）

分支：`feat/access-parity-polish`（基于 `origin/master = a4937f2`）。
对照参考：`/workspace/tiebidli-p1-refs/Access/`（`Access_Core.php`、`Access_UA.php`、`Action.php`、`Plugin.php`）。

## 做了什么

### 1) Feed 归档记日志
- `feed_like`（`src/http.rs`）：comments feed 与各类聚合 feed 在成功返回前调
  `log_archive_visit(ctx, None, None)`（cid/mid 为空，与 PHP `parseArchive` 未识别 feed 一致）；
  feed 未命中走 `not_found`（其内部已记），wrapper 不再记，避免双写。
- 管理员跳过、前端模式不双写的既有门控保留（`log_archive_visit` 内）。

### 2) entrypoint 跟踪
- 新增 `access::resolve_entrypoint(site_base, referer, cookie)`，对齐
  `Access_Core::getEntryPoint`：优先 Referer、空则读 cookie
  `__typecho_access_entrypoint`（cookie 名经 `auth::cookie_name(siteUrl, key)` 加前缀）；
  同 host 视为站内（置空），外链返回 `(entrypoint, true)` 提示调用方写 cookie。
- `log_visit` INSERT 补齐 `entrypoint`、`entrypoint_domain` 列（连同 UA 列，见下）。
- `top_referers` / 控制台「来源域名/来源页」改按 `entrypoint` / `entrypoint_domain`
  聚合（对齐 PHP `parseReferer`），UI 文案与字段形状不变。
- Set-Cookie 回带：backend 日志路径（列表/文章/搜索/404/feed）与 `track.gif`
  前端写库路径都会在需要时写 `__typecho_access_entrypoint` cookie。

### 3) UA 解析 → browser_*/os_*/robot_*
- 新增 `src/access_ua.rs`：完整移植 `Access_UA.php`（50 条 robots 名单、robot 正则、
  OS 十余条链、浏览器近二十条链，含“Linux 遮挡 Ubuntu”等原样行为）。
- `log_visit` INSERT 带上 `browser_id`、`browser_version`、`os_id`、`os_version`、
  `robot`、`robot_id`、`robot_version`；`robot` 不再恒为 0。

### 4) IP 查询 KeyCDN fallback
- 新增 `access::geo_for_ip` / `geo_for_ip_with(ip, keycdn_base)` / `keycdn_lookup`，
  对齐 `Action::ip()`：本地可判定（回环/内网/链路本地/组播/保留）直接 `code 0`；
  公网请求 `https://tools.keycdn.com/geo.json?host={ip}`，成功
  `code 0, data "{country_name} {city}"`，失败
  `code 100, data "很抱歉，ipip.net查询无结果，同时你的服务器无法连接fallback接口(tools.keycdn.com)"`。
- `keycdn_base` 可注入，单测用本地 mock HTTP 服务覆盖成功/失败/500 三形；
  不再硬返回「未内置 IP 归属库」。
- 新增依赖 `reqwest`（`default-features = false` + `rustls-tls-manual-roots`，仅查询用）。

### 5) 概览查询性能
- `overview` 从约 81 次 COUNT（24 小时×3 + 今日/昨日/全站×3）改成固定 **3 条聚合 SQL**：
  Q1 今日按小时 `GROUP BY`（SQLite `CAST((time-?)/3600 AS INTEGER)` /
  MySQL `FLOOR`；UV 用 `COUNT(DISTINCT ip, ua)` / SQLite 用 `ip || CHAR(7) || ua` 拼接等价），
  Q2 今日/昨日按天分组一次出总量，Q3 全站总量。`Overview` 形状不变。

### 6) MySQL 测试
- 单测 `ddl_covers_both_dialects` 断言双发 DDL/索引字符串；`overview`/`log_visit` 逻辑测在 SQLite 内存库跑。
- 新增 `tests/access_mysql.rs`（`#[ignore]`，`TIEBIDLI_MYSQL_URL` 缺省自动跳过）：
  本地 MariaDB 11.8 实测通过（`ensure_table` 幂等 + `log_visit` + 回读断言）。

## 测例

- `cargo test` 全绿：lib 30（含新增 `access_ua` 2、`access` 6），p1 6（含新增
  `access_feed_entrypoint_ua_parity`：feed+1 行且 cid/mid 空、外链 Set-Cookie/同站不写/
  无 Referer 沿用 cookie、Googlebot `robot=1`、Chrome `browser_id`），site 9。
- `TIEBIDLI_MYSQL_URL='mysql://tb:tb@127.0.0.1:3306/tiebidli_test' cargo test --test access_mysql -- --ignored`：1 passed。
- `tests/p1.rs` 的 `8.8.8.8` 断言改为形状断言（`code 0` 或 fallback 文案含 `tools.keycdn.com`），
  不依赖沙箱外网；在无外网沙箱里走 `code 100` 分支。

## 剩余限制

- EntryPoint 的站内判定按 host 相等（端口/子域不同视为外链），与 PHP `parse_url` 口径一致。
- 前端写入模式（`writeType=1`）下打点请求的 Referer 是本站文章页，entrypoint 多为空——与 PHP 同行为。
- MySQL 下 `Any` 驱动解不了 `TINYINT(1)`（sqlx 已知限制）；生产查询只在 WHERE 用 `robot` 列，
  从不 SELECT 它，控制台/筛选不受影响（与 PHP `tinyint(1)` 定义保持一致，未改列类型）。
- KeyCDN 查询超时 8 秒；生产需出站 HTTPS。
