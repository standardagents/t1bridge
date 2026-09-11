use crate::common::{Manifest, Result, digest, mutable, require};

/// The origin must bypass the CDN. Public reads are only made after origin upload.
pub trait Store {
    fn origin(&mut self, name: &str) -> Result<Option<Vec<u8>>>;
    fn put(&mut self, name: &str, bytes: &[u8], immutable: bool) -> Result<()>;
    fn public(&mut self, name: &str) -> Result<Vec<u8>>;
}

pub fn check_baseline(
    store: &mut impl Store,
    baseline: &Manifest,
    desired: &Manifest,
    committing: bool,
) -> Result<()> {
    for name in baseline
        .keys()
        .chain(desired.keys())
        .filter(|name| mutable(name))
    {
        let actual = store.origin(name)?.map(|bytes| digest(&bytes));
        require(
            actual.as_ref() == baseline.get(name)
                || (committing && actual.as_ref() == desired.get(name)),
            format!(
                "origin changed: {name}; retain staging and resolve the conflicting publication"
            ),
        )?;
    }
    Ok(())
}

pub fn upload_immutable(
    store: &mut impl Store,
    assets: &Manifest,
    read: impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<()> {
    for (name, expected) in assets.iter().filter(|(name, _)| !mutable(name)) {
        match store.origin(name)? {
            Some(bytes) => require(
                digest(&bytes) == *expected,
                format!("immutable collision: {name}"),
            )?,
            None => store.put(name, &read(name)?, true)?,
        }
        require(
            digest(&store.public(name)?) == *expected,
            format!("public object differs: {name}; resume verification later"),
        )?;
    }
    Ok(())
}

pub fn upload_mutable(
    store: &mut impl Store,
    assets: &Manifest,
    read: impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<()> {
    // Publish the database aliases last. Object storage cannot atomically swap
    // multiple indexes/signatures; clients may briefly reject mismatched pairs.
    let mut names: Vec<_> = assets.keys().filter(|name| mutable(name)).collect();
    names.sort_by_key(|name| (name.starts_with("standardagents.db"), *name));
    for name in names {
        if store
            .origin(name)?
            .is_none_or(|bytes| digest(&bytes) != assets[name])
        {
            store.put(name, &read(name)?, false)?;
        }
    }
    Ok(())
}

pub fn verify_public(store: &mut impl Store, assets: &Manifest) -> Result<()> {
    for (name, hash) in assets {
        require(
            digest(&store.public(name)?) == *hash,
            format!("public verification failed: {name}; retain staging and resume"),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        origin: BTreeMap<String, Vec<u8>>,
        writes: Vec<String>,
        missing_public: bool,
        fail_write: Option<String>,
    }
    impl Store for Fake {
        fn origin(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.origin.get(name).cloned())
        }
        fn put(&mut self, name: &str, bytes: &[u8], _: bool) -> Result<()> {
            require(
                self.fail_write.as_deref() != Some(name),
                "interrupted upload",
            )?;
            self.writes.push(name.to_string());
            self.origin.insert(name.to_string(), bytes.to_vec());
            Ok(())
        }
        fn public(&mut self, name: &str) -> Result<Vec<u8>> {
            require(!self.missing_public, "cached 404")?;
            self.origin
                .get(name)
                .cloned()
                .ok_or_else(|| "public preflight before upload".into())
        }
    }
    fn assets() -> BTreeMap<String, Vec<u8>> {
        [
            ("a.pkg.tar.zst", b"new package".to_vec()),
            ("standardagents.db", b"new index".to_vec()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }
    fn hashes(bytes: &BTreeMap<String, Vec<u8>>) -> Manifest {
        bytes.iter().map(|(k, v)| (k.clone(), digest(v))).collect()
    }

    #[test]
    fn cached_404_stops_before_indexes_and_resumes_without_reupload() {
        let bytes = assets();
        let hashes = hashes(&bytes);
        let mut store = Fake {
            missing_public: true,
            ..Fake::default()
        };
        assert!(upload_immutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).is_err());
        assert_eq!(store.writes, ["a.pkg.tar.zst"]);
        store.missing_public = false;
        upload_immutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).unwrap();
        upload_mutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).unwrap();
        verify_public(&mut store, &hashes).unwrap();
        assert_eq!(store.writes, ["a.pkg.tar.zst", "standardagents.db"]);
    }

    #[test]
    fn immutable_collision_never_overwrites() {
        let bytes = assets();
        let mut store = Fake::default();
        store
            .origin
            .insert("a.pkg.tar.zst".into(), b"different".to_vec());
        assert!(upload_immutable(&mut store, &hashes(&bytes), |n| Ok(bytes[n].clone())).is_err());
        assert!(store.writes.is_empty());
    }

    #[test]
    fn concurrent_index_change_rejects_even_on_resume() {
        let mut store = Fake::default();
        store
            .origin
            .insert("standardagents.db".into(), b"third publisher".to_vec());
        let baseline = [("standardagents.db".into(), digest(b"old"))].into();
        for committing in [false, true] {
            assert!(check_baseline(&mut store, &baseline, &hashes(&assets()), committing).is_err());
        }
        assert!(store.writes.is_empty());
    }

    #[test]
    fn partial_upload_and_index_commit_resume_exact_staged_bytes() {
        let mut bytes = assets();
        bytes.insert("b.pkg.tar.zst".into(), b"second package".to_vec());
        bytes.insert("standardagents.files".into(), b"files".to_vec());
        let hashes = hashes(&bytes);
        let mut store = Fake {
            fail_write: Some("b.pkg.tar.zst".into()),
            ..Fake::default()
        };
        assert!(upload_immutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).is_err());
        store.fail_write = None;
        upload_immutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).unwrap();
        store.fail_write = Some("standardagents.db".into());
        assert!(upload_mutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).is_err());
        check_baseline(&mut store, &Manifest::new(), &hashes, true).unwrap();
        assert!(check_baseline(&mut store, &Manifest::new(), &hashes, false).is_err());
        store.fail_write = None;
        upload_mutable(&mut store, &hashes, |n| Ok(bytes[n].clone())).unwrap();
        verify_public(&mut store, &hashes).unwrap();
        assert_eq!(
            store.writes,
            [
                "a.pkg.tar.zst",
                "b.pkg.tar.zst",
                "standardagents.files",
                "standardagents.db"
            ]
        );
    }
}
