//! 公开页渲染接口。HTTP 只依赖 [`Renderer`]。
//! 内置实现是默认 MiniJinja 模板。
//! `THEME_DIR` 里的同名 html 在进程启动时覆盖内置文件。
//! 插件不是加载器：见 `plugins` 模块。公开页通过 `plugin_header` / `plugin_footer` 收钩子，模板不写插件名。
//! HelloWorld 只登记后台顶栏，不往公开页页脚写内容。

use crate::model::{Post, Site};
use crate::render::{self, Chrome, SingleView};
use crate::repo::Stats;
use crate::model::Comment;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[allow(dead_code)]
pub struct PageContext<'a> {
    pub chrome: &'a Chrome<'a>,
    pub site: &'a Site,
    pub posts: &'a [Post],
    pub post: Option<&'a Post>,
    pub comments: &'a [Comment],
    pub stats: &'a Stats,
}

pub trait Renderer: Send + Sync {
    fn render_list(
        &self,
        chrome: &Chrome<'_>,
        heading: &str,
        sub: &str,
        posts: &[Post],
        page: i64,
        total: i64,
        base_route: &str,
        page_route: &str,
        route_vals: &HashMap<String, String>,
        show_tag_cloud: bool,
    ) -> Result<String, minijinja::Error>;

    fn render_search(
        &self,
        chrome: &Chrome<'_>,
        keyword: &str,
        posts: &[Post],
        page: i64,
        total: i64,
    ) -> Result<String, minijinja::Error>;

    fn render_single(
        &self,
        chrome: &Chrome<'_>,
        view: &SingleView<'_>,
    ) -> Result<String, minijinja::Error>;

    fn render_404(&self, chrome: &Chrome<'_>) -> Result<String, minijinja::Error>;
}

pub struct JinjaTheme {
    env: minijinja::Environment<'static>,
}

impl JinjaTheme {
    pub fn new(template_dir: Option<&Path>) -> Self {
        Self {
            env: render::environment(template_dir),
        }
    }

    pub fn shared(template_dir: Option<&Path>) -> Arc<dyn Renderer> {
        Arc::new(Self::new(template_dir))
    }
}

impl Renderer for JinjaTheme {
    fn render_list(
        &self,
        chrome: &Chrome<'_>,
        heading: &str,
        sub: &str,
        posts: &[Post],
        page: i64,
        total: i64,
        base_route: &str,
        page_route: &str,
        route_vals: &HashMap<String, String>,
        show_tag_cloud: bool,
    ) -> Result<String, minijinja::Error> {
        render::render_list(
            &self.env,
            chrome,
            heading,
            sub,
            posts,
            page,
            total,
            base_route,
            page_route,
            route_vals,
            show_tag_cloud,
        )
    }

    fn render_search(
        &self,
        chrome: &Chrome<'_>,
        keyword: &str,
        posts: &[Post],
        page: i64,
        total: i64,
    ) -> Result<String, minijinja::Error> {
        render::render_search(&self.env, chrome, keyword, posts, page, total)
    }

    fn render_single(
        &self,
        chrome: &Chrome<'_>,
        view: &SingleView<'_>,
    ) -> Result<String, minijinja::Error> {
        render::render_single(&self.env, chrome, view)
    }

    fn render_404(&self, chrome: &Chrome<'_>) -> Result<String, minijinja::Error> {
        render::render_404(&self.env, chrome)
    }
}
