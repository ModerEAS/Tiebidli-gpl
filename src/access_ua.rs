//! `Access_UA` 的 Rust 移植：浏览器 / 操作系统 / 爬虫三段判定。
//!
//! 对齐参考 `Access/Access_UA.php`：判定顺序、正则与取值逐条对应，
//! 只是把 PHP 的懒解析改成一次算完的 [`ParsedUa`]。

use std::sync::OnceLock;

/// 参考 `Access_UA::$robots`（顺序保留：命中时后者覆盖前者）。
const ROBOTS: &[&str] = &[
    "DuckDuckGo-Favicons-Bot",
    "gce-spider",
    "YisouSpider",
    "YandexBot",
    "UptimeRobot",
    "TencentTraveler",
    "Baiduspider",
    "BaiduGame",
    "Googlebot",
    "msnbot",
    "Sosospider+",
    "Sogou web spider",
    "ia_archiver",
    "Yahoo! Slurp",
    "YoudaoBot",
    "Yahoo Slurp",
    "MSNBot",
    "Java (Often spam bot)",
    "BaiDuSpider",
    "Voila",
    "Yandex bot",
    "BSpider",
    "twiceler",
    "Sogou Spider",
    "Speedy Spider",
    "Google AdSense",
    "Heritrix",
    "Python-urllib",
    "Alexa (IA Archiver)",
    "Ask",
    "Exabot",
    "Custo",
    "OutfoxBot/YodaoBot",
    "yacy",
    "SurveyBot",
    "legs",
    "lwp-trivial",
    "Nutch",
    "StackRambler",
    "The web archive (IA Archiver)",
    "Perl tool",
    "MJ12bot",
    "Netcraft",
    "MSIECrawler",
    "WGet tools",
    "larbin",
    "Fish search",
    "crawler",
    "bingbot",
];

#[derive(Debug, Clone, Default)]
pub struct ParsedUa {
    pub browser_id: String,
    pub browser_version: String,
    pub os_id: String,
    pub os_version: String,
    pub robot: bool,
    pub robot_id: String,
    pub robot_version: String,
}

fn filter(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect()
}

fn re(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("access_ua pattern")
}

fn robot_re() -> &'static regex::Regex {
    static CELL: OnceLock<regex::Regex> = OnceLock::new();
    // 对齐 `#([a-zA-Z0-9]+\s*(?:bot|spider))[ /v]*([0-9.]*)#i`。
    CELL.get_or_init(|| re(r"(?i)([a-zA-Z0-9]+\s*(?:bot|spider))[ /v]*([0-9.]*)"))
}

fn robot_of(ua: &str, ual: &str) -> (String, String) {
    let mut id = String::new();
    let mut version = String::new();
    if !ua.is_empty() {
        if let Some(cap) = robot_re().captures(ua) {
            id = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            version = cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
            for val in ROBOTS {
                if ual.contains(&filter(val)) {
                    id = (*val).to_string();
                }
            }
        } else {
            for val in ROBOTS {
                if ual.contains(&filter(val)) {
                    id = (*val).to_string();
                }
            }
        }
        // 名单命中但正则没抓到版本时，版本留空（与 PHP 一致）。
        if id.is_empty() {
            version.clear();
        }
    }
    (id, version)
}

fn browser_of(ua: &str) -> (String, String) {
    static CELL: OnceLock<Vec<(String, regex::Regex)>> = OnceLock::new();
    let rules = CELL.get_or_init(|| {
        vec![
            ("Camino", r"(?i)(?:Camino|Chimera)[ /]([a-zA-Z0-9.]+)"),
            ("SE 2", r"(?i)SE 2([a-zA-Z0-9.]+)"),
            ("360", r"(?i)360([a-zA-Z0-9.]+)"),
            ("Maxthon", r"(?i)Maxthon( |/)([a-zA-Z0-9.]+)"),
            ("Edge", r"(?i)Edge/([a-zA-Z0-9.]+)"),
            ("Chrome", r"(?i)Chrome/([a-zA-Z0-9.]+)"),
            ("XiaoMi", r"(?i)XiaoMi/MiuiBrowser/([0-9.]+)"),
            ("Safari", r"(?i)Safari/([a-zA-Z0-9.]+)"),
            ("OperaMini", r"(?i)opera mini"),
            ("Opera", r"(?i)Opera.([a-zA-Z0-9.]+)"),
            ("TencentTraveler", r"(?i)TencentTraveler ([a-zA-Z0-9.]+)"),
            ("QQ", r"(?i)QQ/([a-zA-Z0-9.]+)"),
            ("UCWEB", r"(?i)UCWEB([a-zA-Z0-9.]+)"),
            ("UCBrowser", r"(?i)UCBrowser/([a-zA-Z0-9.]+)"),
            ("Quark", r"(?i)Quark/([a-zA-Z0-9.]+)"),
            ("MSIE", r"(?i)MSIE ([a-zA-Z0-9.]+)"),
            ("Trident", r"(?i)Trident"),
            ("Firefox", r"(?i)(?:Firefox|Phoenix|Firebird|BonEcho|GranParadiso|Minefield|Iceweasel)/([a-zA-Z0-9.]+)"),
        ]
        .into_iter()
        .map(|(k, p)| (k.to_string(), re(p)))
        .collect()
    });
    for (key, rx) in rules {
        let Some(cap) = rx.captures(ua) else { continue };
        let ver = |i: usize| cap.get(i).map(|m| m.as_str().to_string()).unwrap_or_default();
        return match key.as_str() {
            // PHP 里 ID 与展示名有别（搜狗/360/腾讯TT/手机QQ/UC/Quark/小米），ID 取英文键。
            "Camino" => ("Camino".into(), ver(1)),
            "SE 2" => ("SE 2".into(), ver(1)),
            "360" => ("360".into(), ver(1)),
            "Maxthon" => ("Maxthon".into(), ver(2)),
            "Edge" => ("Edge".into(), ver(1)),
            "Chrome" => ("Chrome".into(), ver(1)),
            "XiaoMi" => ("小米浏览器".into(), ver(1)),
            "Safari" => ("Safari".into(), ver(1)),
            "OperaMini" => {
                let v = regex::Regex::new(r"(?i)Opera/([a-zA-Z0-9.]+)")
                    .ok()
                    .and_then(|r| r.captures(ua))
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                ("Opera Mini".into(), v)
            }
            "Opera" => ("Opera".into(), ver(1)),
            "TencentTraveler" => ("TencentTraveler".into(), ver(1)),
            "QQ" => ("QQ".into(), ver(1)),
            "UCWEB" => ("UCWEB".into(), ver(1)),
            "UCBrowser" => ("UCBrowser".into(), ver(1)),
            "Quark" => ("Quark".into(), ver(1)),
            "MSIE" => ("Internet Explorer".into(), ver(1)),
            "Trident" => ("Internet Explorer".into(), "11".into()),
            "Firefox" => ("Firefox".into(), ver(1)),
            _ => (String::new(), String::new()),
        };
    }
    (String::new(), String::new())
}

fn os_rules() -> &'static [(regex::Regex, &'static str, OsVer)] {
    static CELL: OnceLock<Vec<(regex::Regex, &'static str, OsVer)>> = OnceLock::new();
    CELL.get_or_init(|| {
        // 顺序即 PHP `parseOS` 的 elseif 链（含 Linux 在 Ubuntu 之前的“遮挡”，照搬）。
        let raw: &[(&str, &'static str, OsVer)] = &[
            (r"(?i)Windows NT 6\.0", "Windows", OsVer::Fixed("Vista")),
            (r"(?i)Windows NT 6\.1", "Windows", OsVer::Fixed("7")),
            (r"(?i)Windows NT 6\.2", "Windows", OsVer::Fixed("8")),
            (r"(?i)Windows NT 6\.3", "Windows", OsVer::Fixed("8.1")),
            (r"(?i)Windows NT 10\.0", "Windows", OsVer::Fixed("10")),
            (r"(?i)Windows NT 5\.0", "Windows", OsVer::Fixed("2000")),
            (r"(?i)Windows NT 5\.1", "Windows", OsVer::Fixed("XP")),
            (r"(?i)Android ([0-9.]+)", "Android", OsVer::Group),
            (r"(?i)iPhone OS ([_0-9]+)", "iPhone OS", OsVer::UnderscoreGroup),
            (r"(?i)iPad; CPU OS ([_0-9]+)", "iPad OS", OsVer::UnderscoreGroup),
            (r"(?i)Mac OS X ([0-9_]+)", "Mac OS X", OsVer::UnderscoreGroup),
            (r"(?i)Linux", "Linux", OsVer::Fixed("")),
            (r"(?i)Ubuntu", "Ubuntu", OsVer::Fixed("")),
        ];
        raw.iter().map(|(p, id, v)| (re(p), *id, *v)).collect()
    })
}

#[derive(Clone, Copy)]
enum OsVer {
    Fixed(&'static str),
    Group,
    UnderscoreGroup,
}

fn os_of(ua: &str) -> (String, String) {
    for (rx, id, ver) in os_rules() {
        let Some(cap) = rx.captures(ua) else { continue };
        let version = match ver {
            OsVer::Fixed(v) => (*v).to_string(),
            OsVer::Group => cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default(),
            OsVer::UnderscoreGroup => cap
                .get(1)
                .map(|m| m.as_str().replace('_', "."))
                .unwrap_or_default(),
        };
        return (id.to_string(), version);
    }
    // Windows 5.2 区分 Win64（PHP 里单独的 if，与上面链条同级，照搬）。
    if re(r"(?i)Windows NT 5\.2").is_match(ua) {
        let version = if re(r"(?i)Win64").is_match(ua) { "XP (64 bit)" } else { "2003" };
        return ("Windows".into(), version.into());
    }
    for (pat, prefix) in [
        (r"(?i)CrOS i686 ([a-zA-Z0-9.]+)", "i686"),
        (r"(?i)CrOS x86_64 ([a-zA-Z0-9.]+)", "x86_64"),
    ] {
        if let Some(cap) = re(pat).captures(ua) {
            let v: String =
                cap.get(1).map(|m| m.as_str().chars().take(4).collect()).unwrap_or_default();
            return ("Chrome OS".into(), format!("{prefix} {v}"));
        }
    }
    (String::new(), String::new())
}

/// 解析一条 UA（空串返回全空，与 PHP `new Access_UA('')` 一致）。
pub fn parse(ua: &str) -> ParsedUa {
    let ual = filter(ua);
    let (robot_id, robot_version) = robot_of(ua, &ual);
    let (browser_id, browser_version) = browser_of(ua);
    let (os_id, os_version) = os_of(ua);
    ParsedUa {
        browser_id,
        browser_version,
        os_id,
        os_version,
        robot: !robot_id.is_empty(),
        robot_id,
        robot_version,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn robots_detected() {
        for ua in [
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "Mozilla/5.0 (compatible; Baiduspider/2.0; +http://www.baidu.com/search/spider.html)",
            "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)",
            "Python-urllib/3.11",
            "msnbot/2.0b (+http://search.msn.com/msnbot.htm)",
            "MJ12bot/v1.4.8 (http://mj12bot.com/)",
        ] {
            let p = parse(ua);
            assert!(p.robot, "{ua}");
            assert!(!p.robot_id.is_empty(), "{ua}");
        }
        let g = parse("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)");
        assert_eq!(g.robot_version, "2.1");
    }

    #[test]
    fn browsers_and_os() {
        let chrome = parse("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36");
        assert!(!chrome.robot);
        assert_eq!(chrome.browser_id, "Chrome");
        assert_eq!(chrome.browser_version, "126.0.0.0");
        assert_eq!(chrome.os_id, "Windows");
        assert_eq!(chrome.os_version, "10");

        let iphone = parse("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1");
        assert_eq!(iphone.browser_id, "Safari");
        assert_eq!(iphone.os_id, "iPhone OS");
        assert_eq!(iphone.os_version, "17.0");

        assert!(parse("").robot_id.is_empty());
        assert!(!parse("").robot);
    }
}
