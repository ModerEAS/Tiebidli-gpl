//! PHP `serialize()` 的一个子集解析器。
//! Typecho 1.2 把 `routingTable`、`theme:{name}` 等选项存成 PHP 序列化字符串。
//! 只支持 `N/b/i/d/s/a`。对象（`O`/`C`）会返回错误，调用方退回默认值。

#[derive(Debug, Clone, PartialEq)]
pub enum PhpValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<(PhpValue, PhpValue)>),
}

impl PhpValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PhpValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn to_string_value(&self) -> Option<String> {
        match self {
            PhpValue::String(s) => Some(s.clone()),
            PhpValue::Int(n) => Some(n.to_string()),
            PhpValue::Bool(b) => Some(if *b { "1" } else { "0" }.into()),
            PhpValue::Float(n) => Some(n.to_string()),
            _ => None,
        }
    }

    pub fn get_key(&self, key: &str) -> Option<&PhpValue> {
        let PhpValue::Array(items) = self else {
            return None;
        };
        items
            .iter()
            .find(|(k, _)| match k {
                PhpValue::String(s) => s == key,
                PhpValue::Int(n) => n.to_string() == key,
                _ => false,
            })
            .map(|(_, v)| v)
    }

    pub fn string_list(&self) -> Vec<String> {
        let PhpValue::Array(items) = self else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|(_, v)| v.to_string_value())
            .collect()
    }
}

pub fn serialize(value: &PhpValue) -> String {
    serialize_value(value)
}

pub fn serialize_string_map(entries: &[(&str, PhpValue)]) -> String {
    let mut body = String::new();
    for (k, v) in entries {
        body.push_str(&serialize_string(k));
        body.push_str(&serialize_value(v));
    }
    format!("a:{}:{{{body}}}", entries.len())
}

fn serialize_value(v: &PhpValue) -> String {
    match v {
        PhpValue::Null => "N;".into(),
        PhpValue::Bool(b) => format!("b:{};", if *b { 1 } else { 0 }),
        PhpValue::Int(n) => format!("i:{n};"),
        PhpValue::Float(n) => format!("d:{n};"),
        PhpValue::String(s) => serialize_string(s),
        PhpValue::Array(items) => {
            let mut body = String::new();
            for (k, v) in items {
                body.push_str(&serialize_value(k));
                body.push_str(&serialize_value(v));
            }
            format!("a:{}:{{{body}}}", items.len())
        }
    }
}

fn serialize_string(s: &str) -> String {
    format!("s:{}:\"{s}\";", s.len())
}

pub fn parse_php(input: &str) -> Result<PhpValue, String> {
    let mut p = Parser {
        s: input.as_bytes(),
        i: 0,
    };
    let v = p.parse()?;
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a [u8] {
        &self.s[self.i..]
    }

    fn eat(&mut self, lit: &[u8]) -> Result<(), String> {
        if !self.rest().starts_with(lit) {
            return Err(format!(
                "php serialize: expected {}, got {}",
                String::from_utf8_lossy(lit),
                String::from_utf8_lossy(&self.rest()[..self.rest().len().min(16)])
            ));
        }
        self.i += lit.len();
        Ok(())
    }

    fn parse_int(&mut self) -> Result<i64, String> {
        let start = self.i;
        if self.i < self.s.len() && self.s[self.i] == b'-' {
            self.i += 1;
        }
        let digits = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        if self.i == digits {
            return Err("php serialize: missing integer".into());
        }
        std::str::from_utf8(&self.s[start..self.i])
            .map_err(|e| e.to_string())?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())
    }

    fn parse(&mut self) -> Result<PhpValue, String> {
        let tag = *self.rest().first().ok_or("php serialize: eof")?;
        match tag {
            b'N' => {
                self.eat(b"N;")?;
                Ok(PhpValue::Null)
            }
            b'b' => {
                self.eat(b"b:")?;
                let n = self.parse_int()?;
                self.eat(b";")?;
                Ok(PhpValue::Bool(n != 0))
            }
            b'i' => {
                self.eat(b"i:")?;
                let n = self.parse_int()?;
                self.eat(b";")?;
                Ok(PhpValue::Int(n))
            }
            b'd' => {
                self.eat(b"d:")?;
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b';' {
                    self.i += 1;
                }
                let num = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0");
                self.eat(b";")?;
                let f = if num.eq_ignore_ascii_case("NAN") {
                    f64::NAN
                } else if num.eq_ignore_ascii_case("INF") {
                    f64::INFINITY
                } else if num.eq_ignore_ascii_case("-INF") {
                    f64::NEG_INFINITY
                } else {
                    num.parse().unwrap_or(0.0)
                };
                Ok(PhpValue::Float(f))
            }
            b's' => {
                self.eat(b"s:")?;
                let len = self.parse_int()?;
                if len < 0 {
                    return Err("php serialize: negative string length".into());
                }
                self.eat(b":\"")?;
                let len = len as usize;
                if self.i + len > self.s.len() {
                    return Err("php serialize: short string".into());
                }
                let bytes = self.s[self.i..self.i + len].to_vec();
                self.i += len;
                self.eat(b"\";")?;
                let s = String::from_utf8(bytes)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                Ok(PhpValue::String(s))
            }
            b'a' => {
                self.eat(b"a:")?;
                let n = self.parse_int()?;
                if n < 0 {
                    return Err("php serialize: negative array".into());
                }
                self.eat(b":{")?;
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let k = self.parse()?;
                    let v = self.parse()?;
                    items.push((k, v));
                }
                self.eat(b"}")?;
                Ok(PhpValue::Array(items))
            }
            other => Err(format!(
                "php serialize: unsupported type '{}'",
                other as char
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_route_fragment() {
        let raw = r#"a:1:{s:4:"post";a:3:{s:3:"url";s:21:"/archives/[slug].html";s:6:"widget";s:15:"\Widget\Archive";s:6:"action";s:6:"render";}}"#;
        let v = parse_php(raw).unwrap();
        let url = v
            .get_key("post")
            .and_then(|p| p.get_key("url"))
            .and_then(|u| u.as_str())
            .unwrap();
        assert_eq!(url, "/archives/[slug].html");
    }

    #[test]
    fn parses_scalars_and_utf8() {
        let v = parse_php(r#"a:3:{s:4:"name";s:6:"随笔";s:1:"n";i:-3;s:1:"b";b:1;}"#).unwrap();
        assert_eq!(v.get_key("name").and_then(|x| x.as_str()), Some("随笔"));
        assert_eq!(v.get_key("n"), Some(&PhpValue::Int(-3)));
        assert_eq!(v.get_key("b"), Some(&PhpValue::Bool(true)));
    }
}
