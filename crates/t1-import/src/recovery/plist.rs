//! Bounded XML/binary plists for Apple's signing and restore protocols.
//! XML never resolves a DTD or loads an external entity. Duplicate dictionary
//! keys are errors rather than an opportunity to override a validated field.

use super::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;
pub(super) use t1_bridge::bplist::Value;
use t1_bridge::bplist::{self, MAX_PLIST_SIZE};

const INVALID: Error = Error("invalid recovery property list");

pub(super) fn dict<const N: usize>(items: [(&str, Value); N]) -> Value {
    Value::Dictionary(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

pub(super) fn string(value: &str) -> Value {
    Value::String(value.into())
}

pub(super) fn get<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    dictionary(value)?.get(key).ok_or(INVALID)
}

pub(super) fn dictionary(value: &Value) -> Result<&BTreeMap<String, Value>> {
    match value {
        Value::Dictionary(v) => Ok(v),
        _ => Err(INVALID),
    }
}

pub(super) fn text(value: &Value) -> Result<&str> {
    match value {
        Value::String(v) => Ok(v),
        _ => Err(INVALID),
    }
}

pub(super) fn integer(value: &Value) -> Result<u64> {
    match value {
        Value::Integer(v) => u64::try_from(*v).map_err(|_| INVALID),
        Value::String(v) => v.strip_prefix("0x").map_or_else(
            || v.parse().map_err(|_| INVALID),
            |hex| u64::from_str_radix(hex, 16).map_err(|_| INVALID),
        ),
        _ => Err(INVALID),
    }
}

pub(super) fn data(value: &Value) -> Result<&[u8]> {
    match value {
        Value::Data(v) if !v.is_empty() => Ok(v),
        _ => Err(INVALID),
    }
}

struct Element {
    name: Vec<u8>,
    text: String,
    children: Vec<(bool, Value)>,
}

impl Element {
    fn finish(self) -> Result<(bool, Value)> {
        let container = matches!(self.name.as_slice(), b"plist" | b"dict" | b"array");
        if (container && !self.text.trim().is_empty()) || (!container && !self.children.is_empty())
        {
            return Err(INVALID);
        }
        let value = match self.name.as_slice() {
            b"key" | b"string" => Value::String(self.text),
            b"integer" => {
                let value: i128 = self.text.trim().parse().map_err(|_| INVALID)?;
                if value < i128::from(i64::MIN) || value > i128::from(u64::MAX) {
                    return Err(INVALID);
                }
                Value::Integer(value)
            }
            b"data" => {
                let compact: String = self
                    .text
                    .chars()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                Value::Data(STANDARD.decode(compact).map_err(|_| INVALID)?)
            }
            b"true" | b"false" if self.text.trim().is_empty() => {
                Value::Boolean(self.name == b"true")
            }
            b"array" if self.children.iter().all(|(key, _)| !key) => {
                Value::Array(self.children.into_iter().map(|(_, v)| v).collect())
            }
            b"dict" if self.children.len().is_multiple_of(2) => {
                let mut map = BTreeMap::new();
                let mut children = self.children.into_iter();
                while let Some((true, Value::String(key))) = children.next() {
                    let (false, value) = children.next().ok_or(INVALID)? else {
                        return Err(INVALID);
                    };
                    if map.insert(key, value).is_some() {
                        return Err(INVALID);
                    }
                }
                Value::Dictionary(map)
            }
            b"plist" if self.children.len() == 1 && !self.children[0].0 => {
                self.children.into_iter().next().ok_or(INVALID)?.1
            }
            _ => return Err(INVALID),
        };
        Ok((self.name == b"key", value))
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_PLIST_SIZE {
        return Err(INVALID);
    }
    if bytes.starts_with(b"bplist00") {
        return bplist::decode(bytes).map_err(|_| INVALID);
    }
    let source = std::str::from_utf8(bytes).map_err(|_| INVALID)?;
    let mut reader = Reader::from_str(source);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut output = None;
    let mut count = 0_u32;
    loop {
        match reader.read_event().map_err(|_| INVALID)? {
            Event::Start(tag) => {
                if stack.len() >= 64 || output.is_some() || count >= 262_144 {
                    return Err(INVALID);
                }
                count += 1;
                if stack.is_empty() && tag.name().as_ref() != "plist" {
                    return Err(INVALID);
                }
                if tag.name().as_ref() != "plist" && tag.attributes().next().is_some() {
                    return Err(INVALID);
                }
                stack.push(Element {
                    name: tag.name().as_ref().as_bytes().to_vec(),
                    text: String::new(),
                    children: vec![],
                });
            }
            Event::End(_) => {
                let element = stack.pop().ok_or(INVALID)?;
                if element.name == b"dict"
                    && element
                        .children
                        .iter()
                        .enumerate()
                        .any(|(i, (key, _))| *key != (i % 2 == 0))
                {
                    return Err(INVALID);
                }
                let value = element.finish()?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(value);
                } else {
                    output = Some(value.1);
                }
            }
            Event::Text(t) => {
                let text = t.as_ref();
                if let Some(parent) = stack.last_mut() {
                    parent.text.push_str(text);
                } else if !text.trim().is_empty() {
                    return Err(INVALID);
                }
            }
            Event::GeneralRef(reference) => {
                let entity = format!("&{};", reference.as_ref());
                let expanded = quick_xml::escape::unescape(&entity).map_err(|_| INVALID)?;
                stack.last_mut().ok_or(INVALID)?.text.push_str(&expanded);
            }
            Event::DocType(t)
                if stack.is_empty() && output.is_none() && !t.as_ref().contains('[') => {}
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof if stack.is_empty() => return output.ok_or(INVALID),
            _ => return Err(INVALID),
        }
    }
}

pub(super) fn encode_xml(value: &Value) -> Result<Vec<u8>> {
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\">",
    );
    write_value(value, &mut output, 0)?;
    output.push_str("</plist>");
    if output.len() > MAX_PLIST_SIZE {
        return Err(INVALID);
    }
    Ok(output.into_bytes())
}

fn write_value(value: &Value, output: &mut String, depth: usize) -> Result<()> {
    if depth >= 64 || output.len() > MAX_PLIST_SIZE {
        return Err(INVALID);
    }
    match value {
        Value::Boolean(true) => output.push_str("<true/>"),
        Value::Boolean(false) => output.push_str("<false/>"),
        Value::String(value) => tagged(output, "string", &quick_xml::escape::escape(value)),
        Value::Integer(value) => tagged(output, "integer", &value.to_string()),
        Value::Data(value) => tagged(output, "data", &STANDARD.encode(value)),
        Value::Array(values) => {
            output.push_str("<array>");
            for value in values {
                write_value(value, output, depth + 1)?;
            }
            output.push_str("</array>");
        }
        Value::Dictionary(values) => {
            output.push_str("<dict>");
            for (key, value) in values {
                tagged(output, "key", &quick_xml::escape::escape(key));
                write_value(value, output, depth + 1)?;
            }
            output.push_str("</dict>");
        }
        Value::Null => return Err(INVALID),
    }
    Ok(())
}

fn tagged(output: &mut String, tag: &str, value: &str) {
    output.push('<');
    output.push_str(tag);
    output.push('>');
    output.push_str(value);
    output.push_str("</");
    output.push_str(tag);
    output.push('>');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_restore_values_in_both_encodings() {
        let value = dict([(
            "Synthetic&Key",
            Value::Array(vec![
                string("<synthetic>"),
                Value::Data(vec![0, 255]),
                Value::Boolean(true),
                Value::Integer(u64::MAX.into()),
            ]),
        )]);
        assert_eq!(decode(&encode_xml(&value).unwrap()).unwrap(), value);
        assert_eq!(decode(&bplist::encode(&value).unwrap()).unwrap(), value);
    }

    #[test]
    fn rejects_duplicates_entities_shape_and_depth() {
        for body in [
            "<dict><key>x</key><true/><key>x</key><false/></dict>",
            "<dict><true/><false/></dict>",
            "<dict><key>x</key></dict>",
            "<string>&unknown;</string>",
            "<array><key>x</key></array>",
            "<integer>18446744073709551616</integer>",
            "<data>@@</data>",
        ] {
            assert!(decode(format!("<plist>{body}</plist>").as_bytes()).is_err());
        }
        assert!(decode(b"<!DOCTYPE plist [<!ENTITY x SYSTEM 'file:///synthetic'>]><plist><string>&x;</string></plist>").is_err());
        assert!(
            decode(
                format!(
                    "<plist>{}<true/>{}</plist>",
                    "<array>".repeat(65),
                    "</array>".repeat(65)
                )
                .as_bytes()
            )
            .is_err()
        );
        assert!(decode(b"<plist><true/></plist><plist><false/></plist>").is_err());
    }
}
