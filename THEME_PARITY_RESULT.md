# 前台模板结果（THEME_DIR 方案）

分支：`feat/theme-parity-frontend`。对照现网（快照 `/workspace/miaostay-inspect/*.html`、
主题 PHP 结构）只迁前台在用的部分；**样式全部自写占位**，视觉细化后续再做。

## 启用

- 模板：`THEME_DIR=<repo>/themes/handsome`（5 个 `.html` 在启动时覆盖内置同名模板）。
- 静态：`THEME_STATIC_DIR=<repo>/themes/handsome`，
  `/usr/themes/handsome/assets/...` 即 `assets/...`（目录名与现网 `options.theme` 一致，
  对齐静态路径；后台「外观」页会列出该目录）。
- 后台把 `options.theme` 设为 `handsome`（或保持默认，外观只影响静态前缀）。

## 预览（液态玻璃演示样式）

- 同上启用后，用浏览器打开首页/文章页即可看到玻璃效果：
  `assets/css/style.css` 为自写液态玻璃样式（半透明 + 模糊 + 高光描边）；
  右上角「暗色」开关可切换 `body.dark` 玻璃质感；无 `backdrop-filter` 的浏览器自动实色回退。
- 真液态玻璃版：面板 `::before` 层做 `backdrop-filter` 模糊 + SVG
  `feTurbulence`/`feDisplacementMap` 折射（`base.html` 内联自写 filter 节点，
  正文不进 filter 保持可读），`::after` 层做 28s 循环流动高光，渐变描边代替单线边框。

## 落地（引擎侧薄能力）

- 友链注入：公开渲染上下文新增 `friend_links`（Links 启用时取 `ten` 分组；
  注释 `<!--使用links插件，输出全站友链-->` 与现网同位置），默认侧栏同步加「友情链接」节。
- 浏览数：`post_card` 新增 `views`（列表卡片 `浏览次数` + `meta-value`）；
  侧栏新增「热门文章」：Access 启用时按 `views`（`hot_posts_by_views`，列缺失回退），
  否则沿用评论数排序。
- 点赞：`GET/POST /?action=star_post&cid=`、`star_talk&coid=`，纯文本 `1/2/-1`
 （cookie `extend_post_stars` / `extend_say_stars` 去重，计数列 `contents.stars` /
  `comments.stars` 缺列 best-effort 补）。
- 音乐：`GET /action/handsome-meting-api`（与现网 LocalConst `MUSIC_API` 同路径，
  主题 Action 非引擎能力）；参数校验对齐参考实现；目前只实现 `netease` 源
 （song/playlist/artist/album/search/url/lrc/pic），其它源回 403。
  `?do=parse` 需登录，返回 `[hplayer]` 短代码文本（与参考同形）。
  前端默认不配歌单（`window.TB_MUSIC` 未设置时播放器隐藏），配好即播。
- 时光机：`POST /action/cross-edit?do=edit|delete&coid=&_=`，登录 + contributor 门禁，
  `_` 绑定来源页（与后台 `_` 同机制），作者本人或编辑可改自己的评论；
  `nav_slug == cross` 的页面渲染评论流（含 `star_talk`、编辑入口、点赞数）。
- 上下文新增：`hot_posts`、`recent_posts`、`recent_comments`、`categories`（名/链接/计数）、
  `archives_list`、`stars`（单页当前数）、`has_math`（正文含公式才加载 MathJax）、
  `cross_comments`、`meting_api*`、`calendar_json`/`cats_json`（占位数据）。

## 落地（模板侧骨架）

`themes/handsome/`：`base/list/single/search/404.html` + `assets/css/style.css`（占位，
只保布局：顶栏固定、侧栏左置、主列避让、暗色/阅读模式类）+ `assets/js/site.js`（自写胶水）。
对齐的 class/节点：`app-header/navbar/header_left/header_right`、`app-aside/navi`、
`app-content/app-content-body`（PJAX 容器 `#content`）、`app-footer`、`search_modal`、
`blog-post post-list`、`single-post panel`、`index-post-img/item-thumb`、`post-meta`、
`post_title_wrapper/index-post-title`、`summary`、`post-item-foot`、`meta-views/meta-value`、
`entry-content`、`tocTree`、`support-author/like_button/like_label`、`star_post`、
`star_talk/star_count/say_footer/panel-footer`、`post-calendar/category-radar`、
`widget-tabs-4-hots`、`handsome_global_player`、`respond-*` 评论表单。

## 刻意跳过

- 主题作者 CSS（`handsome.min.css` 等）与闭源 `core.min.js` / `function.min.js`：
  **未提交任何原文件**，样式/胶水全部自写。
- 追番/bilifan/douban、空 GitHub 日历、坏 SW、后台 MultiUpload、未引用的 EditorMD/Mailer。
- `[secret]` 短代码（README 刻意不做；时光机正文原样输出）。
- PHP `functions.php` 执行、动态扫 `usr/plugins`（架构不做）。
- ECharts：日历/雷达只做有数据的占位列表，不引 ECharts。
- 评论树保持引擎现有 class（未逐条改成主题自定义结构），时光机流除外。

## 剩余限制

- 音乐只实现网易源；上游不可达时 Action 回 502（`music api error`），前台静默不播。
- `Any` 驱动读 MySQL `TINYINT` 的已知限制与之前一致（点赞计数查询只在 SQLite/默认路径验证过；
  MySQL 真库测沿用上次结论）。
- 顶栏登录表单为普通链接（未做 AJAX 登录），Cloudflare 统计需自行加模板片段。
