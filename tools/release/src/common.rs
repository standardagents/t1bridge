use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub type Manifest = BTreeMap<String, String>;

pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .flat_map(|byte| {
            let alphabet = b"0123456789abcdef";
            [
                char::from(alphabet[usize::from(byte >> 4)]),
                char::from(alphabet[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

#[allow(clippy::case_sensitive_file_extension_comparisons)] // Repository object names are case-sensitive.
pub fn mutable(name: &str) -> bool {
    name.starts_with("standardagents.db")
        || name.starts_with("standardagents.files")
        || name.starts_with("SHA256SUMS")
        || name.ends_with(".PKGBUILD")
        || name.ends_with(".patch")
        || name.ends_with(".asc")
        || name == "PRERELEASE.md"
}

pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let mut result = Manifest::new();
    for line in std::str::from_utf8(bytes)?.lines() {
        let (hash, name) = line.split_once("  ").ok_or("invalid checksum line")?;
        require(
            hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && safe_name(name),
            "invalid checksum or filename",
        )?;
        require(
            result
                .insert(name.to_string(), hash.to_lowercase())
                .is_none(),
            "duplicate checksum filename",
        )?;
    }
    require(!result.is_empty(), "empty manifest")?;
    Ok(result)
}

pub fn manifest_bytes(manifest: &Manifest) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, hash) in manifest {
        bytes.extend_from_slice(hash.as_bytes());
        bytes.extend_from_slice(b"  ");
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

pub fn run(
    program: &str,
    args: &[&str],
    directory: &Path,
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(directory)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(bytes) = input {
        child
            .stdin
            .take()
            .ok_or("missing command stdin")?
            .write_all(bytes)?;
    }
    let output = child.wait_with_output()?;
    require(
        output.status.success(),
        format!("{program} failed ({})", output.status),
    )?;
    Ok(output.stdout)
}

pub fn path_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| "non-UTF8 path".into())
}

pub fn read_regular(path: &Path) -> Result<Vec<u8>> {
    require(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected regular file",
    )?;
    Ok(fs::read(path)?)
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    fs::File::open(path.parent().ok_or("missing parent")?)?.sync_all()?;
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub account: String,
    pub bucket: String,
    pub prefix: String,
    pub public_url: String,
    pub repository: String,
    pub tag: String,
    pub checkout: String,
    pub keyring: String,
    pub primary_fingerprint: String,
    pub signing_fingerprint: String,
    pub signer: String,
    pub notes_file: String,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        require(
            self.account.len() == 32 && self.account.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid account",
        )?;
        require(
            safe_name(&self.bucket)
                && self.prefix.ends_with('/')
                && self.prefix.trim_end_matches('/').split('/').all(safe_name),
            "invalid object location",
        )?;
        require(
            self.public_url.starts_with("https://")
                && self.public_url.ends_with('/')
                && !self.public_url.contains(['\n', '\r', '"', '?', '#', '@']),
            "invalid public URL",
        )?;
        require(
            self.repository.split('/').count() == 2
                && self.repository.split('/').all(safe_name)
                && safe_name(&self.tag),
            "invalid repository or tag",
        )?;
        for fingerprint in [&self.primary_fingerprint, &self.signing_fingerprint] {
            require(
                fingerprint.len() == 40 && fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid fingerprint",
            )?;
        }
        for path in [
            &self.checkout,
            &self.keyring,
            &self.signer,
            &self.notes_file,
        ] {
            require(
                Path::new(path).is_absolute(),
                "configuration paths must be absolute",
            )?;
        }
        Ok(())
    }

    pub fn verify(&self, directory: &Path, name: &str) -> Result<()> {
        let status = run(
            "gpgv",
            &[
                "--status-fd=1",
                "--keyring",
                &self.keyring,
                &format!("{name}.sig"),
                name,
            ],
            directory,
            None,
        )?;
        let pinned = String::from_utf8(status)?.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.get(1) == Some(&"VALIDSIG")
                && fields
                    .last()
                    .is_some_and(|f| f.eq_ignore_ascii_case(&self.primary_fingerprint))
        });
        require(
            pinned,
            format!("signature for {name} is not from the pinned key"),
        )
    }

    pub fn sign(&self, directory: &Path, name: &str) -> Result<()> {
        run(
            &self.signer,
            &[
                "--batch",
                "--yes",
                "--local-user",
                &format!("{}!", self.signing_fingerprint),
                "--output",
                &format!("{name}.sig"),
                "--detach-sign",
                name,
            ],
            directory,
            None,
        )?;
        self.verify(directory, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_rejects_traversal_duplicates_and_malformed_hashes() {
        let hash = digest(b"fixture");
        let valid = format!("{hash}  synthetic.pkg.tar.zst\n");
        assert_eq!(
            manifest_bytes(&parse_manifest(valid.as_bytes()).unwrap()),
            valid.as_bytes()
        );
        for invalid in [
            format!("{hash}  ../escape\n"),
            format!("{valid}{valid}"),
            "not-a-hash  package\n".into(),
            String::new(),
        ] {
            assert!(parse_manifest(invalid.as_bytes()).is_err());
        }
    }
}
