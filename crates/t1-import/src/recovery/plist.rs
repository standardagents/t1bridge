//! Bounded XML/binary plists for Apple's signing and restore protocols.
//! XML never resolves a DTD or loads an external entity. Duplicate dictionary
//! keys are errors rather than an opportunity to override a validated field.

use super::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;
pub(super) use t1_bridge::bplist::Value;
use t1_bridge::bplist::{self, MAX_PLIST_SIZE};

// Restore component replies contain base64-encoded firmware. Keep their outbound
// XML allowance separate from the 16 MiB inbound/TSS/binary-plist limit. The
// four-byte framing prefix must also fit the mux channel's 32 MiB queue.
pub(super) const MAX_RESTORE_XML_SIZE: usize = 32 * 1024 * 1024 - 4;

pub(super) fn dict<const N: usize>(items: [(&str, Value); N]) -> Value {
    Value::Dictionary(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

pub(super) fn string(value: &str) -> Value {
    Value::String(value.into())
}

pub(super) fn get<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    dictionary(value)?.get(key).ok_or_else(|| {
        // Only fixed protocol field names may appear in diagnostics.
        if matches!(
            key,
            "MsgType" | "DataType" | "Command" | "ConnPort" | "Status"
        ) {
            eprintln!("recovery: required protocol field absent: {key}");
            if let Value::Dictionary(fields) = value {
                for name in ["Result", "Error", "Status"] {
                    if fields.contains_key(name) {
                        eprintln!("recovery: response contains protocol field {name}");
                    }
                }
                if let Some(Value::String(result)) = fields.get("Result")
                    && matches!(result.as_str(), "Success" | "Failure" | "Error")
                {
                    eprintln!("recovery: response Result is {result}");
                }
            }
        }
        Error("recovery property list is missing a required field")
    })
}

pub(super) fn dictionary(value: &Value) -> Result<&BTreeMap<String, Value>> {
    match value {
        Value::Dictionary(v) => Ok(v),
        _ => Err(Error("recovery property list expected a dictionary")),
    }
}

pub(super) fn text(value: &Value) -> Result<&str> {
    match value {
        Value::String(v) => Ok(v),
        _ => Err(Error("recovery property list expected a string")),
    }
}

pub(super) fn integer(value: &Value) -> Result<u64> {
    match value {
        Value::Integer(v) => u64::try_from(*v)
            .map_err(|_| Error("property-list integer is outside the unsigned range")),
        Value::String(v) => v.strip_prefix("0x").map_or_else(
            || {
                v.parse()
                    .map_err(|_| Error("property-list string is not a decimal integer"))
            },
            |hex| {
                u64::from_str_radix(hex, 16)
                    .map_err(|_| Error("property-list string is not a hexadecimal integer"))
            },
        ),
        _ => Err(Error("recovery property list expected an integer")),
    }
}

pub(super) fn data(value: &Value) -> Result<&[u8]> {
    match value {
        Value::Data(v) if !v.is_empty() => Ok(v),
        _ => Err(Error("recovery property list expected nonempty data")),
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
            return Err(Error("XML container has text or scalar has children"));
        }
        let value = match self.name.as_slice() {
            b"key" | b"string" => Value::String(self.text),
            b"integer" => {
                let value: i128 = self
                    .text
                    .trim()
                    .parse()
                    .map_err(|_| Error("XML integer is not decimal"))?;
                if value < i128::from(i64::MIN) || value > i128::from(u64::MAX) {
                    return Err(Error("XML integer is outside the supported range"));
                }
                Value::Integer(value)
            }
            b"real" => return Err(Error("recovery XML contains an unsupported real value")),
            b"date" => return Err(Error("recovery XML contains an unsupported date value")),
            b"data" => {
                let compact: String = self
                    .text
                    .chars()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                Value::Data(
                    STANDARD
                        .decode(compact)
                        .map_err(|_| Error("XML data has invalid base64 encoding"))?,
                )
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
                    let (false, value) = children
                        .next()
                        .ok_or(Error("XML dictionary has a key without a value"))?
                    else {
                        return Err(Error("XML dictionary has a key in a value position"));
                    };
                    if map.insert(key, value).is_some() {
                        return Err(Error("XML dictionary has a duplicate key"));
                    }
                }
                Value::Dictionary(map)
            }
            b"plist" if self.children.len() == 1 && !self.children[0].0 => {
                self.children
                    .into_iter()
                    .next()
                    .ok_or(Error("XML plist root is empty"))?
                    .1
            }
            _ => return Err(Error("XML element has an unsupported name or shape")),
        };
        Ok((self.name == b"key", value))
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Value> {
    decode_inner(bytes).inspect_err(|_| {
        let binary = bytes.starts_with(b"bplist00");
        let trailing_nul = bytes.last() == Some(&0);
        eprintln!(
            "recovery: rejected property list: bytes={} binary={binary} trailing_nul={trailing_nul}",
            bytes.len()
        );
    })
}

fn decode_inner(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_PLIST_SIZE {
        return Err(Error("property list exceeds the size limit"));
    }
    if bytes.starts_with(b"bplist00") {
        return bplist::decode(bytes).map_err(|error| {
            // This codec's error Display is payload-redacted by contract.
            eprintln!("recovery: binary property-list decoding failed: {error}");
            Error("binary property-list decoding failed")
        });
    }
    let source = std::str::from_utf8(bytes).map_err(|_| Error("XML property list is not UTF-8"))?;
    let mut reader = Reader::from_str(source);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut output = None;
    let mut count = 0_u32;
    loop {
        match reader
            .read_event()
            .map_err(|_| Error("XML reader rejected the document syntax"))?
        {
            Event::Start(tag) => {
                if stack.len() >= 64 || output.is_some() || count >= 262_144 {
                    return Err(Error(
                        "XML document exceeds nesting or element limits or has multiple roots",
                    ));
                }
                count += 1;
                if stack.is_empty() && tag.name().as_ref() != "plist" {
                    return Err(Error("XML root is not a plist element"));
                }
                if tag.name().as_ref() != "plist" && tag.attributes().next().is_some() {
                    return Err(Error("XML non-root element has attributes"));
                }
                stack.push(Element {
                    name: tag.name().as_ref().as_bytes().to_vec(),
                    text: String::new(),
                    children: vec![],
                });
            }
            Event::End(_) => {
                let element = stack
                    .pop()
                    .ok_or(Error("XML closing element has no open element"))?;
                if element.name == b"dict"
                    && element
                        .children
                        .iter()
                        .enumerate()
                        .any(|(i, (key, _))| *key != (i % 2 == 0))
                {
                    return Err(Error("XML dictionary keys and values do not alternate"));
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
                    return Err(Error("XML has non-whitespace text outside the root"));
                }
            }
            Event::GeneralRef(reference) => {
                let entity = format!("&{};", reference.as_ref());
                let expanded = quick_xml::escape::unescape(&entity)
                    .map_err(|_| Error("XML contains an unsupported entity reference"))?;
                stack
                    .last_mut()
                    .ok_or(Error("XML entity reference is outside the root"))?
                    .text
                    .push_str(&expanded);
            }
            Event::CData(_) => {
                return Err(Error("recovery XML contains unsupported CDATA"));
            }
            Event::DocType(t)
                if stack.is_empty() && output.is_none() && !t.as_ref().contains('[') => {}
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof if stack.is_empty() => {
                return output.ok_or(Error("XML document contains no completed root"));
            }
            _ => return Err(Error("XML has an unsupported event or unclosed root")),
        }
    }
}

pub(super) fn encode_xml(value: &Value) -> Result<Vec<u8>> {
    encode_xml_bounded(value, MAX_PLIST_SIZE)
}

pub(super) fn encode_restore_xml(value: &Value) -> Result<Vec<u8>> {
    encode_xml_bounded(value, MAX_RESTORE_XML_SIZE)
}

fn encode_xml_bounded(value: &Value, limit: usize) -> Result<Vec<u8>> {
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\">",
    );
    write_value(value, &mut output, 0, limit)?;
    output.push_str("</plist>");
    if output.len() > limit {
        return Err(Error("encoded XML property list exceeds the size limit"));
    }
    Ok(output.into_bytes())
}

fn write_value(value: &Value, output: &mut String, depth: usize, limit: usize) -> Result<()> {
    if depth >= 64 || output.len() > limit {
        return Err(Error("XML encoding exceeds depth or size limits"));
    }
    match value {
        Value::Boolean(true) => output.push_str("<true/>"),
        Value::Boolean(false) => output.push_str("<false/>"),
        Value::String(value) => tagged(output, "string", &quick_xml::escape::escape(value)),
        Value::Integer(value) => tagged(output, "integer", &value.to_string()),
        Value::Data(value) => {
            // Reject oversized blobs before allocating their base64 expansion.
            let length = base64::encoded_len(value.len(), true)
                .and_then(|n| n.checked_add("<data></data>".len()))
                .ok_or(Error("XML data encoding size overflow"))?;
            if length > limit - output.len() {
                return Err(Error("encoded XML property list exceeds the size limit"));
            }
            tagged(output, "data", &STANDARD.encode(value));
        }
        Value::Array(values) => {
            output.push_str("<array>");
            for value in values {
                write_value(value, output, depth + 1, limit)?;
            }
            output.push_str("</array>");
        }
        Value::Dictionary(values) => {
            output.push_str("<dict>");
            for (key, value) in values {
                tagged(output, "key", &quick_xml::escape::escape(key));
                write_value(value, output, depth + 1, limit)?;
            }
            output.push_str("</dict>");
        }
        Value::Null => return Err(Error("XML property lists cannot encode a null value")),
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
    fn restore_xml_allows_firmware_without_raising_inbound_or_signing_limits() {
        let raw = vec![0x5a; 13 * 1024 * 1024];
        let reply = dict([("KernelCacheFile", Value::Data(raw.clone()))]);
        assert!(encode_xml(&reply).is_err());
        let xml = encode_restore_xml(&reply).unwrap();
        assert!(xml.len() > MAX_PLIST_SIZE && xml.len() <= MAX_RESTORE_XML_SIZE);
        assert!(decode(&xml).is_err());
        let text = std::str::from_utf8(&xml).unwrap();
        let data = text
            .split_once("<data>")
            .unwrap()
            .1
            .split_once("</data>")
            .unwrap()
            .0;
        assert_eq!(STANDARD.decode(data).unwrap(), raw);
        assert!(
            encode_restore_xml(&dict([(
                "KernelCacheFile",
                Value::Data(vec![0; 24 * 1024 * 1024])
            )]))
            .is_err()
        );
    }

    #[test]
    fn xml_limits_include_wrappers_and_aggregate_data() {
        let value = dict([("Synthetic", Value::Data(vec![1, 2, 3]))]);
        let xml = encode_xml(&value).unwrap();
        assert_eq!(encode_xml_bounded(&value, xml.len()).unwrap(), xml);
        assert!(encode_xml_bounded(&value, xml.len() - 1).is_err());
        let values = Value::Array(vec![Value::Data(vec![1; 100]), Value::Data(vec![2; 100])]);
        let length = encode_xml(&values).unwrap().len();
        assert!(encode_xml_bounded(&values, length - 1).is_err());
        assert!(encode_xml_bounded(&values, length).is_ok());
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
