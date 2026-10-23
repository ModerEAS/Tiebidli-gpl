//! 后台「插件」页上的功能分组。
//!
//! 这里的插件就是普通的进程内功能：和站点其它代码编进同一个二进制，
//! 启动时放进静态登记表 `builtin_plugins()`。启用、停用只改 `options`，
//! 并决定程序里已经写好的钩子会不会跑。边界是组织形式（trait、登记表、options），
//! 用来做和 Typecho 一样的管理入口。
//!
//! 没有动态加载。不加载 DLL，不 `dlopen` `.so`，不在运行时载入插件 crate，也不跑 WASM。
//! 不扫描 `usr/plugins`，也不执行 PHP。功能仍写在原来的 Rust 模块里；
//! 只有需要出现在插件管理页时才登记到这里。
//!
//! 启用状态写在 `options.plugins`，形状与 Typecho `Plugin::export()` 相同：
//! `activated` 是插件名到钩子回调的映射，`handles` 是钩子名到回调列表。
//! 插件自己的设置写在 `plugin:{Name}`，PHP 序列化。禁用时删掉 `plugin:{Name}` 和 `_plugin:{Name}`。

use crate::model::Site;
use crate::phpval::{self, PhpValue};
use crate::text::html_escape;
use crate::db::Db;

const NAV_HANDLE: &str = "admin/menu.php:navBar";
const HELLO_CALLBACK: &str = r"TypechoPlugin\HelloWorld\Plugin::render";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// `Widget\Archive:header`，插在公开页 `<head>` 末尾。
    PublicHeader,
    /// `Widget\Archive:footer`，插在公开页页脚。HelloWorld 不登记这个钩子。
    PublicFooter,
    /// `admin/menu.php:navBar`，插在后台顶栏用户名前面。
    AdminNavBar,
    /// 文章 HTML 出来之后再过一遍。没有插件登记时原文返回。
    Content,
}

pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default_value: &'static str,
}

pub struct Binding {
    pub point: Point,
    /// 写入 `options.plugins` 的钩子名。`None` 表示只在这个程序里跑，不写成 PHP 回调。
    pub php_handle: Option<&'static str>,
    pub php_callback: Option<&'static str>,
}

pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn author(&self) -> &'static str;
    fn homepage(&self) -> &'static str;
    fn config_fields(&self) -> &'static [Field];
    fn personal_fields(&self) -> &'static [Field] {
        &[]
    }
    fn bindings(&self) -> &'static [Binding];
    fn snippet(&self, point: Point, site: &Site) -> String;
    fn filter_content(&self, _site: &Site, html: &str) -> String {
        html.to_string()
    }
}

struct HelloWorld;

impl Plugin for HelloWorld {
    fn name(&self) -> &'static str {
        "HelloWorld"
    }
    fn title(&self) -> &'static str {
        "HelloWorld"
    }
    fn description(&self) -> &'static str {
        "Hello World"
    }
    fn version(&self) -> &'static str {
        "1.0.0"
    }
    fn author(&self) -> &'static str {
        "qining"
    }
    fn homepage(&self) -> &'static str {
        "http://typecho.org"
    }
    fn config_fields(&self) -> &'static [Field] {
        &[Field {
            name: "word",
            label: "说点什么",
            description: "",
            default_value: "Hello World",
        }]
    }
    fn bindings(&self) -> &'static [Binding] {
        &[Binding {
            point: Point::AdminNavBar,
            php_handle: Some(NAV_HANDLE),
            php_callback: Some(HELLO_CALLBACK),
        }]
    }
    fn snippet(&self, point: Point, site: &Site) -> String {
        match point {
            Point::AdminNavBar => {
                let word = setting(site, self.name(), "word", "Hello World");
                format!(
                    r#"<span class="message success">{}</span>"#,
                    html_escape(&word)
                )
            }
            _ => String::new(),
        }
    }
}

pub fn builtin_plugins() -> &'static [&'static dyn Plugin] {
    &[&HelloWorld]
}

pub fn manage_page(site: &Site) -> String {
    let activated = activated_names(site.get("plugins"));
    let mut on_rows = String::new();
    let mut any_builtin_on = false;
    for plugin in builtin_plugins() {
        if !activated.iter().any(|name| name == plugin.name()) {
            continue;
        }
        any_builtin_on = true;
        on_rows.push_str(&plugin_row(*plugin, true));
    }
    for name in &activated {
        if builtin_plugins().iter().any(|plugin| plugin.name() == name) {
            continue;
        }
        on_rows.push_str(&missing_row(name));
    }
    let mut off_rows = String::new();
    for plugin in builtin_plugins() {
        if activated.iter().any(|name| name == plugin.name()) {
            continue;
        }
        off_rows.push_str(&plugin_row(*plugin, false));
    }
    let mut html = String::from(r#"<div class="row typecho-page-main" role="main"><div class="col-mb-12 typecho-list">"#);
    if any_builtin_on || activated.iter().any(|name| builtin_plugins().iter().all(|plugin| plugin.name() != name)) {
        html.push_str(&table_block("启用的插件", &on_rows, false));
    }
    if !off_rows.is_empty() || !any_builtin_on {
        if off_rows.is_empty() {
            off_rows = r#"<tr><td colspan="5"><h6 class="typecho-list-table-title">没有安装插件</h6></td></tr>"#.into();
        }
        html.push_str(&table_block("禁用的插件", &off_rows, true));
    }
    html.push_str(
        r#"<p class="description">这里列出的功能都编译进同一程序。启用和停用只改选项，并决定已经写好的钩子会不会运行。不能把 PHP 目录放进 usr/plugins 指望它运行。</p>"#,
    );
    html.push_str("</div></div>");
    html
}

pub fn config_page(site: &Site, name: &str) -> Result<(String, String), &'static str> {
    if name.is_empty() {
        return Err("插件不存在");
    }
    let Some(plugin) = find(name) else {
        return Err("无法配置插件");
    };
    if plugin.config_fields().is_empty() || !is_activated(site, name) {
        return Err("无法配置插件");
    }
    let mut fields = String::new();
    for field in plugin.config_fields() {
        let value = setting(site, name, field.name, field.default_value);
        let desc = if field.description.is_empty() {
            String::new()
        } else {
            format!(
                r#"<p class="description">{}</p>"#,
                html_escape(field.description)
            )
        };
        fields.push_str(&format!(
            r#"<li><label class="typecho-label" for="{id}">{label}</label><input id="{id}" name="{id}" type="text" class="text w-100" value="{value}">{desc}</li>"#,
            id = html_escape(field.name),
            label = html_escape(field.label),
            value = html_escape(&value),
        ));
    }
    let action = format!("/action/plugins-edit?config={}", urlencoding::encode(name));
    let html = format!(
        r#"<div class="row typecho-page-main" role="form"><div class="col-mb-12 col-tb-8 col-tb-offset-2"><form method="post" action="{action}"><ul class="typecho-option">{fields}</ul><ul class="typecho-option"><li class="typecho-option-submit"><button type="submit" class="btn primary">保存设置</button></li></ul></form></div></div>"#
    );
    Ok((format!("设置插件 {}", plugin.title()), html))
}

pub fn personal_html(site: &Site) -> String {
    let mut html = String::new();
    for plugin in builtin_plugins() {
        if plugin.personal_fields().is_empty() || !is_activated(site, plugin.name()) {
            continue;
        }
        html.push_str(&format!("<p>{}</p>", html_escape(plugin.title())));
    }
    html
}

pub fn public_header(site: &Site) -> String {
    snippets(site, Point::PublicHeader)
}

pub fn public_footer(site: &Site) -> String {
    snippets(site, Point::PublicFooter)
}

pub fn admin_nav(site: &Site) -> String {
    snippets(site, Point::AdminNavBar)
}

pub fn filter_content(site: &Site, html: &str) -> String {
    let names = activated_names(site.get("plugins"));
    let mut html = html.to_string();
    for plugin in builtin_plugins() {
        if !names.iter().any(|name| name == plugin.name()) {
            continue;
        }
        if plugin.bindings().iter().any(|binding| binding.point == Point::Content) {
            html = plugin.filter_content(site, &html);
        }
    }
    html
}

pub async fn activate(db: &Db, site: &Site, name: &str) -> Result<&'static str, &'static str> {
    let Some(plugin) = find(name) else {
        return Err("无法启用插件");
    };
    let mut root = load_root(site.get("plugins"), "无法启用插件")?;
    let mut activated = child_array(&root, "activated");
    if activated.iter().any(|(key, _)| key_eq(key, name)) {
        return Err("无法启用插件");
    }
    let recorded = recorded_handles(plugin);
    activated.push((
        PhpValue::String(name.to_string()),
        PhpValue::Array(vec![(
            PhpValue::String("handles".into()),
            PhpValue::Array(callback_groups(&recorded)),
        )]),
    ));
    let mut handles = child_array(&root, "handles");
    merge_handles(&mut handles, &recorded);
    set_child(&mut root, "activated", PhpValue::Array(activated));
    set_child(&mut root, "handles", PhpValue::Array(handles));
    db.set_option("plugins", &phpval::serialize(&PhpValue::Array(root)))
        .await
        .map_err(|_| "无法启用插件")?;
    if !plugin.config_fields().is_empty() && site.get(&option_name(name)).is_empty() {
        let entries: Vec<(&str, PhpValue)> = plugin
            .config_fields()
            .iter()
            .map(|field| (field.name, PhpValue::String(field.default_value.into())))
            .collect();
        db.set_option(&option_name(name), &phpval::serialize_string_map(&entries))
            .await
            .map_err(|_| "无法启用插件")?;
    }
    Ok("插件已经被启用")
}

pub async fn deactivate(db: &Db, site: &Site, name: &str) -> Result<&'static str, &'static str> {
    if name.is_empty() || name.contains(['&', '?', '"', '<', '>', ' ']) {
        return Err("无法禁用插件");
    }
    let mut root = load_root(site.get("plugins"), "无法禁用插件")?;
    let mut activated = child_array(&root, "activated");
    let Some(index) = activated.iter().position(|(key, _)| key_eq(key, name)) else {
        return Err("无法禁用插件");
    };
    let callbacks = callbacks_of(&activated[index].1);
    activated.remove(index);
    let mut handles = child_array(&root, "handles");
    for (component, callback) in callbacks {
        strip_callback(&mut handles, &component, &callback);
    }
    set_child(&mut root, "activated", PhpValue::Array(activated));
    set_child(&mut root, "handles", PhpValue::Array(handles));
    db.set_option("plugins", &phpval::serialize(&PhpValue::Array(root)))
        .await
        .map_err(|_| "无法禁用插件")?;
    let _ = db.delete_option_all(&option_name(name)).await;
    let _ = db.delete_option_all(&personal_option_name(name)).await;
    Ok("插件已经被禁用")
}

pub async fn save_config(
    db: &Db,
    site: &Site,
    name: &str,
    form: &std::collections::HashMap<String, String>,
) -> Result<(), &'static str> {
    let Some(plugin) = find(name) else {
        return Err("无法配置插件");
    };
    if plugin.config_fields().is_empty() || !is_activated(site, name) {
        return Err("无法配置插件");
    }
    let raw = site.get(&option_name(name));
    let mut items = if raw.is_empty() {
        Vec::new()
    } else {
        match phpval::parse_php(raw) {
            Ok(PhpValue::Array(items)) => items,
            _ => Vec::new(),
        }
    };
    for field in plugin.config_fields() {
        if let Some(value) = form.get(field.name) {
            upsert_string(&mut items, field.name, value.clone());
        }
    }
    db.set_option(&option_name(name), &phpval::serialize(&PhpValue::Array(items)))
        .await
        .map_err(|_| "无法配置插件")?;
    Ok(())
}

fn find(name: &str) -> Option<&'static dyn Plugin> {
    builtin_plugins()
        .iter()
        .copied()
        .find(|plugin| plugin.name() == name)
}

fn is_activated(site: &Site, name: &str) -> bool {
    activated_names(site.get("plugins"))
        .iter()
        .any(|item| item == name)
}

fn snippets(site: &Site, point: Point) -> String {
    let names = activated_names(site.get("plugins"));
    let mut html = String::new();
    for plugin in builtin_plugins() {
        if !names.iter().any(|name| name == plugin.name()) {
            continue;
        }
        if plugin.bindings().iter().any(|binding| binding.point == point) {
            html.push_str(&plugin.snippet(point, site));
        }
    }
    html
}

fn recorded_handles(plugin: &dyn Plugin) -> Vec<(&'static str, &'static str)> {
    plugin
        .bindings()
        .iter()
        .filter_map(|binding| match (binding.php_handle, binding.php_callback) {
            (Some(handle), Some(callback)) => Some((handle, callback)),
            _ => None,
        })
        .collect()
}

fn callback_groups(recorded: &[(&str, &str)]) -> Vec<(PhpValue, PhpValue)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (handle, callback) in recorded {
        if let Some(group) = groups.iter_mut().find(|(name, _)| name == handle) {
            group.1.push((*callback).to_string());
        } else {
            groups.push(((*handle).to_string(), vec![(*callback).to_string()]));
        }
    }
    groups
        .into_iter()
        .map(|(handle, callbacks)| {
            let items = callbacks
                .into_iter()
                .enumerate()
                .map(|(index, callback)| (PhpValue::Int(index as i64), PhpValue::String(callback)))
                .collect();
            (PhpValue::String(handle), PhpValue::Array(items))
        })
        .collect()
}

fn merge_handles(handles: &mut Vec<(PhpValue, PhpValue)>, recorded: &[(&str, &str)]) {
    for (component, callback) in recorded {
        let mut list = child_array(handles, component);
        if list.iter().any(|(_, value)| value.as_str() == Some(*callback)) {
            continue;
        }
        let key = list
            .iter()
            .filter_map(|(key, _)| match key {
                PhpValue::Int(n) => Some(*n),
                _ => None,
            })
            .max()
            .map(|n| n + 1)
            .unwrap_or(0);
        list.push((PhpValue::Int(key), PhpValue::String((*callback).to_string())));
        set_child(handles, component, PhpValue::Array(list));
    }
}

fn callbacks_of(value: &PhpValue) -> Vec<(String, String)> {
    let PhpValue::Array(items) = value else {
        return Vec::new();
    };
    let handles = child_array(items, "handles");
    let mut found = Vec::new();
    for (component, callbacks) in handles {
        let Some(component) = component.as_str().map(str::to_string).or_else(|| match component {
            PhpValue::Int(n) => Some(n.to_string()),
            _ => None,
        }) else {
            continue;
        };
        let PhpValue::Array(callbacks) = callbacks else {
            continue;
        };
        for (_, callback) in callbacks {
            if let Some(callback) = callback.as_str() {
                found.push((component.clone(), callback.to_string()));
            }
        }
    }
    found
}

fn strip_callback(handles: &mut Vec<(PhpValue, PhpValue)>, component: &str, callback: &str) {
    let mut list = child_array(handles, component);
    if list.is_empty() && handles.iter().all(|(key, _)| !key_eq(key, component)) {
        return;
    }
    list.retain(|(_, value)| value.as_str() != Some(callback));
    if list.is_empty() {
        handles.retain(|(key, _)| !key_eq(key, component));
    } else {
        set_child(handles, component, PhpValue::Array(list));
    }
}

fn option_name(name: &str) -> String {
    format!("plugin:{name}")
}

fn personal_option_name(name: &str) -> String {
    format!("_plugin:{name}")
}

fn setting(site: &Site, plugin: &str, field: &str, default_value: &str) -> String {
    let raw = site.get(&option_name(plugin));
    if raw.is_empty() {
        return default_value.to_string();
    }
    phpval::parse_php(raw)
        .ok()
        .and_then(|value| value.get_key(field).and_then(PhpValue::to_string_value))
        .unwrap_or_else(|| default_value.to_string())
}

fn plugin_row(plugin: &dyn Plugin, enabled: bool) -> String {
    let author = if plugin.homepage().is_empty() {
        html_escape(plugin.author())
    } else {
        format!(
            r#"<a href="{}">{}</a>"#,
            html_escape(plugin.homepage()),
            html_escape(plugin.author())
        )
    };
    let action = if enabled {
        let mut links = String::new();
        if !plugin.config_fields().is_empty() {
            links.push_str(&format!(
                r#"<a href="/admin/options-plugin.php?config={}">设置</a> &bull; "#,
                urlencoding::encode(plugin.name())
            ));
        }
        links.push_str(&format!(
            r#"<a lang="你确认要禁用插件 {name} 吗?" href="/action/plugins-edit?deactivate={name}">禁用</a>"#,
            name = html_escape(plugin.name()),
        ));
        links
    } else {
        format!(
            r#"<a href="/action/plugins-edit?activate={}">启用</a>"#,
            html_escape(plugin.name())
        )
    };
    format!(
        r#"<tr id="plugin-{name}"><td>{title}</td><td>{desc}</td><td class="kit-hidden-mb">{version}</td><td class="kit-hidden-mb">{author}</td><td>{action}</td></tr>"#,
        name = html_escape(plugin.name()),
        title = html_escape(plugin.title()),
        desc = html_escape(plugin.description()),
        version = html_escape(plugin.version()),
        author = author,
    )
}

fn missing_row(name: &str) -> String {
    format!(
        r#"<tr id="plugin-{name}"><td>{label}</td><td colspan="3"><span class="warning">此插件文件已经损坏或者被不安全移除, 强烈建议你禁用它</span></td><td><a lang="你确认要禁用插件 {label} 吗?" href="/action/plugins-edit?deactivate={enc}">禁用</a></td></tr>"#,
        name = html_escape(name),
        label = html_escape(name),
        enc = urlencoding::encode(name),
    )
}

fn table_block(title: &str, rows: &str, deactivated: bool) -> String {
    let class = if deactivated {
        "typecho-list-table deactivate"
    } else {
        "typecho-list-table"
    };
    format!(
        r#"<h4 class="typecho-list-table-title">{title}</h4><div class="typecho-table-wrap"><table class="{class}"><colgroup><col width="25%"/><col width="45%"/><col width="8%" class="kit-hidden-mb"/><col width="10%" class="kit-hidden-mb"/><col width=""/></colgroup><thead><tr><th>名称</th><th>描述</th><th class="kit-hidden-mb">版本</th><th class="kit-hidden-mb">作者</th><th>操作</th></tr></thead><tbody>{rows}</tbody></table></div>"#
    )
}

fn activated_names(raw: &str) -> Vec<String> {
    let Ok(PhpValue::Array(root)) = phpval::parse_php(raw) else {
        return Vec::new();
    };
    child_array(&root, "activated")
        .into_iter()
        .filter_map(|(key, _)| match key {
            PhpValue::String(name) => Some(name),
            PhpValue::Int(n) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

fn load_root(raw: &str, fail: &'static str) -> Result<Vec<(PhpValue, PhpValue)>, &'static str> {
    if raw.is_empty() {
        return Ok(vec![
            (PhpValue::String("activated".into()), PhpValue::Array(vec![])),
            (PhpValue::String("handles".into()), PhpValue::Array(vec![])),
        ]);
    }
    match phpval::parse_php(raw) {
        Ok(PhpValue::Array(items)) => Ok(items),
        _ => Err(fail),
    }
}

fn child_array(items: &[(PhpValue, PhpValue)], key: &str) -> Vec<(PhpValue, PhpValue)> {
    items
        .iter()
        .find(|(item_key, _)| key_eq(item_key, key))
        .and_then(|(_, value)| match value {
            PhpValue::Array(inner) => Some(inner.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn set_child(items: &mut Vec<(PhpValue, PhpValue)>, key: &str, value: PhpValue) {
    if let Some(index) = items.iter().position(|(item_key, _)| key_eq(item_key, key)) {
        items[index].1 = value;
    } else {
        items.push((PhpValue::String(key.to_string()), value));
    }
}

fn upsert_string(items: &mut Vec<(PhpValue, PhpValue)>, key: &str, value: String) {
    if let Some(index) = items.iter().position(|(item_key, _)| key_eq(item_key, key)) {
        items[index].1 = PhpValue::String(value);
    } else {
        items.push((PhpValue::String(key.to_string()), PhpValue::String(value)));
    }
}

fn key_eq(key: &PhpValue, expected: &str) -> bool {
    match key {
        PhpValue::String(value) => value == expected,
        PhpValue::Int(n) => n.to_string() == expected,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::site_from_parts;
    use std::collections::HashMap;

    fn site_with(opts: HashMap<String, String>) -> Site {
        site_from_parts(opts, vec![], vec![], vec![])
    }

    #[test]
    fn hello_world_row_matches_typecho_activate_shape() {
        let site = site_with(HashMap::new());
        let mut root = load_root(site.get("plugins"), "无法启用插件").unwrap();
        let plugin = find("HelloWorld").unwrap();
        let recorded = recorded_handles(plugin);
        let mut activated = child_array(&root, "activated");
        activated.push((
            PhpValue::String("HelloWorld".into()),
            PhpValue::Array(vec![(
                PhpValue::String("handles".into()),
                PhpValue::Array(callback_groups(&recorded)),
            )]),
        ));
        let mut handles = child_array(&root, "handles");
        merge_handles(&mut handles, &recorded);
        set_child(&mut root, "activated", PhpValue::Array(activated));
        set_child(&mut root, "handles", PhpValue::Array(handles));
        let raw = phpval::serialize(&PhpValue::Array(root));
        let expected = concat!(
            r#"a:2:{s:9:"activated";a:1:{s:10:"HelloWorld";a:1:{s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}}s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}"#
        );
        assert_eq!(raw, expected);
        assert_eq!(activated_names(&raw), vec!["HelloWorld".to_string()]);
        assert!(!raw.contains("footer"), "{raw}");
    }

    #[test]
    fn hooks_follow_activated_list_and_saved_word() {
        let off = site_with(HashMap::new());
        assert!(admin_nav(&off).is_empty());
        assert!(public_footer(&off).is_empty());
        assert_eq!(filter_content(&off, "<p>正文</p>"), "<p>正文</p>");

        let mut opts = HashMap::new();
        opts.insert(
            "plugins".into(),
            concat!(
                r#"a:2:{s:9:"activated";a:1:{s:10:"HelloWorld";a:1:{s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}}s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}"#
            )
            .into(),
        );
        opts.insert(
            "plugin:HelloWorld".into(),
            r#"a:1:{s:4:"word";s:12:"你好插件";}"#.into(),
        );
        let on = site_with(opts);
        let nav = admin_nav(&on);
        assert!(nav.contains("你好插件"), "{nav}");
        assert!(nav.contains("message success"), "{nav}");
        assert!(public_footer(&on).is_empty(), "{nav}");
        assert_eq!(filter_content(&on, "<p>正文</p>"), "<p>正文</p>");
        let page = manage_page(&on);
        assert!(page.contains("启用的插件"), "{page}");
        assert!(page.contains("禁用"), "{page}");
        assert!(!page.contains(">启用</a>"), "{page}");
    }

    #[test]
    fn deactivate_keeps_other_plugin_handles() {
        let raw = concat!(
            r#"a:2:{s:9:"activated";a:2:{s:3:"Foo";a:1:{s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:32:"TypechoPlugin\Foo\Plugin::render";}}}s:10:"HelloWorld";a:1:{s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:1:{i:0;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}}s:7:"handles";a:1:{s:21:"admin/menu.php:navBar";a:2:{i:0;s:32:"TypechoPlugin\Foo\Plugin::render";i:1;s:39:"TypechoPlugin\HelloWorld\Plugin::render";}}}"#
        );
        let mut root = load_root(raw, "无法禁用插件").unwrap();
        let mut activated = child_array(&root, "activated");
        let index = activated
            .iter()
            .position(|(key, _)| key_eq(key, "HelloWorld"))
            .unwrap();
        let callbacks = callbacks_of(&activated[index].1);
        activated.remove(index);
        let mut handles = child_array(&root, "handles");
        for (component, callback) in callbacks {
            strip_callback(&mut handles, &component, &callback);
        }
        set_child(&mut root, "activated", PhpValue::Array(activated));
        set_child(&mut root, "handles", PhpValue::Array(handles));
        let next = phpval::serialize(&PhpValue::Array(root));
        assert!(next.contains("Foo"), "{next}");
        assert!(!next.contains("HelloWorld"), "{next}");
        assert!(next.contains(r"TypechoPlugin\Foo\Plugin::render"), "{next}");
        assert!(!next.contains("HelloWorld\\Plugin"), "{next}");
        let names = activated_names(&next);
        assert_eq!(names, vec!["Foo".to_string()]);
        let page = manage_page(&site_with(HashMap::from([("plugins".into(), next)])));
        assert!(page.contains("此插件文件已经损坏或者被不安全移除"), "{page}");
        assert!(page.contains("HelloWorld"), "{page}");
        assert!(page.contains("禁用的插件"), "{page}");
    }
}
