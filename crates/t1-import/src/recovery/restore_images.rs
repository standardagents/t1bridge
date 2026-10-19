//! Manifest-selected firmware-update images requested by restored.

use super::{
    Error, Result, firmware,
    plist::{self, Value},
};

pub(super) fn fud_data(
    request: &Value,
    identity: &Value,
    component: impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<Value> {
    let fields = plist::dictionary(request)?;
    let arguments = fields.get("Arguments").map(plist::dictionary).transpose()?;
    let list = match arguments.and_then(|a| a.get("FUDImageList")) {
        Some(Value::Boolean(value)) => *value,
        None => false,
        _ => return Err(Error("invalid FUD image-list request")),
    };
    let names = firmware::fud_components(identity)?;
    if list {
        return Ok(plist::dict([(
            "FUDImageList",
            Value::Array(names.into_iter().map(plist::string).collect()),
        )]));
    }
    if let Some(name) = arguments.and_then(|a| a.get("ImageName")) {
        let name = plist::text(name)?;
        if !names.contains(&name) {
            return Err(Error(
                "FUD image is absent from the verified firmware manifest",
            ));
        }
        return Ok(plist::dict([
            ("FUDImageData", Value::Data(component(name)?)),
            ("ImageName", plist::string(name)),
        ]));
    }
    let images = names
        .into_iter()
        .map(|name| Ok((name.to_owned(), Value::Data(component(name)?))))
        .collect::<Result<_>>()?;
    Ok(plist::dict([("FUDImageData", Value::Dictionary(images))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Value {
        plist::dict([(
            "Manifest",
            plist::dict([
                (
                    "SyntheticFud",
                    plist::dict([(
                        "Info",
                        plist::dict([("IsFUDFirmware", Value::Boolean(true))]),
                    )]),
                ),
                ("SyntheticOther", plist::dict([("Info", plist::dict([]))])),
            ]),
        )])
    }

    #[test]
    fn list_selects_only_manifest_fud_images_without_loading_bytes() {
        let request = plist::dict([(
            "Arguments",
            plist::dict([("FUDImageList", Value::Boolean(true))]),
        )]);
        let reply = fud_data(&request, &identity(), |_| {
            panic!("list must not load images")
        })
        .unwrap();
        assert_eq!(
            reply,
            plist::dict([(
                "FUDImageList",
                Value::Array(vec![plist::string("SyntheticFud")])
            )])
        );
    }

    #[test]
    fn named_and_bulk_replies_use_the_required_distinct_shapes() {
        let request = plist::dict([(
            "Arguments",
            plist::dict([("ImageName", plist::string("SyntheticFud"))]),
        )]);
        let load = |name: &str| {
            assert_eq!(name, "SyntheticFud");
            Ok(vec![1, 2, 3])
        };
        assert_eq!(
            fud_data(&request, &identity(), load).unwrap(),
            plist::dict([
                ("FUDImageData", Value::Data(vec![1, 2, 3])),
                ("ImageName", plist::string("SyntheticFud")),
            ])
        );
        assert_eq!(
            fud_data(&plist::dict([]), &identity(), load).unwrap(),
            plist::dict([(
                "FUDImageData",
                plist::dict([("SyntheticFud", Value::Data(vec![1, 2, 3]))])
            ),])
        );
    }

    #[test]
    fn rejects_unflagged_images_and_propagates_missing_signed_data() {
        let request = plist::dict([(
            "Arguments",
            plist::dict([("ImageName", plist::string("SyntheticOther"))]),
        )]);
        assert!(
            fud_data(&request, &identity(), |_| panic!(
                "unflagged image must not be loaded"
            ))
            .is_err()
        );
        assert!(
            fud_data(&plist::dict([]), &identity(), |_| Err(Error(
                "synthetic missing signed component"
            )))
            .is_err()
        );
        let empty = plist::dict([("Manifest", plist::dict([]))]);
        assert_eq!(
            fud_data(&plist::dict([]), &empty, |_| panic!("empty manifest")).unwrap(),
            plist::dict([("FUDImageData", plist::dict([]))])
        );
    }
}
