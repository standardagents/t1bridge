//! Exercise archive metadata and pinned signatures with disposable synthetic data.
use crate::{
    common::{Config, digest, path_str, run},
    inventory,
    stage::hash_directory,
};
use std::{fs, path::Path};

#[test]
#[allow(clippy::too_many_lines)] // One disposable signing/archive boundary scenario.
fn signed_repository_rejects_tampering_wrong_key_and_inventory_mismatch() {
    let nonce = fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap();
    let directory = std::env::temp_dir().join(format!("t1-release-verify-{}", nonce.trim()));
    fs::create_dir(&directory).unwrap();
    let home = directory.join("gnupg");
    fs::create_dir(&home).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let gpg = |args: &[&str]| {
        let mut full = vec![
            "--homedir",
            path_str(&home).unwrap(),
            "--batch",
            "--yes",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
        ];
        full.extend_from_slice(args);
        run("gpg", &full, &directory, None).unwrap()
    };
    gpg(&[
        "--quick-generate-key",
        "Synthetic Release <release@example.invalid>",
        "ed25519",
        "sign",
        "1d",
    ]);
    let listing = gpg(&["--with-colons", "--list-keys"]);
    let listing = std::str::from_utf8(&listing).unwrap();
    let fingerprint = listing
        .lines()
        .find(|line| line.starts_with("fpr:"))
        .unwrap()
        .split(':')
        .nth(9)
        .unwrap();
    let assets = directory.join("assets");
    fs::create_dir(&assets).unwrap();
    let fixture = directory.join("fixture");
    fs::create_dir(&fixture).unwrap();
    let filename = "synthetic-package-1-1-any.pkg.tar.zst";
    fs::write(fixture.join(".PKGINFO"), b"pkgname = synthetic-package\npkgbase = synthetic-package\npkgver = 1-1\npkgdesc = Synthetic fixture\nurl = https://example.invalid\nbuilddate = 1\npackager = Synthetic\nsize = 7\narch = any\nlicense = MIT\n").unwrap();
    fs::write(fixture.join("payload"), b"fixture").unwrap();
    run(
        "bsdtar",
        &[
            "--zstd",
            "-cf",
            path_str(&assets.join(filename)).unwrap(),
            ".PKGINFO",
            "payload",
        ],
        &fixture,
        None,
    )
    .unwrap();
    gpg(&["--detach-sign", path_str(&assets.join(filename)).unwrap()]);
    run(
        "repo-add",
        &["--include-sigs", "standardagents.db.tar.zst", filename],
        &assets,
        None,
    )
    .unwrap();
    for kind in ["db", "files"] {
        let alias = assets.join(format!("standardagents.{kind}"));
        fs::remove_file(&alias).unwrap();
        fs::copy(
            assets.join(format!("standardagents.{kind}.tar.zst")),
            &alias,
        )
        .unwrap();
        gpg(&["--detach-sign", path_str(&alias).unwrap()]);
    }
    let mut config = Config {
        account: String::new(),
        bucket: String::new(),
        prefix: String::new(),
        public_url: String::new(),
        repository: String::new(),
        tag: String::new(),
        checkout: String::new(),
        keyring: path_str(&home.join("pubring.kbx")).unwrap().into(),
        primary_fingerprint: fingerprint.into(),
        signing_fingerprint: fingerprint.into(),
        signer: String::new(),
        notes_file: String::new(),
    };
    let manifest = hash_directory(&assets).unwrap();
    let inventory = inventory::verify_repository(&config, &assets, &manifest).unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory["synthetic-package"].version, "1-1");
    config.primary_fingerprint = "0000000000000000000000000000000000000000".into();
    assert!(inventory::verify_repository(&config, &assets, &manifest).is_err());
    config.primary_fingerprint = fingerprint.into();
    fs::write(assets.join(filename), b"tampered package").unwrap();
    let mut rehashed = manifest.clone();
    rehashed.insert(filename.into(), digest(b"tampered package"));
    assert!(inventory::verify_repository(&config, &assets, &manifest).is_err());
    assert!(inventory::verify_repository(&config, &assets, &rehashed).is_err());
    // A valid signature on different content still cannot satisfy the index.
    gpg(&["--detach-sign", path_str(&assets.join(filename)).unwrap()]);
    assert!(
        inventory::verify_repository(&config, &assets, &hash_directory(&assets).unwrap()).is_err()
    );
    run(
        "gpgconf",
        &["--homedir", path_str(&home).unwrap(), "--kill", "all"],
        Path::new("."),
        None,
    )
    .unwrap();
    fs::remove_dir_all(directory).unwrap();
}
