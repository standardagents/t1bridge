use crate::common::{Config, Result, path_str, require, run, safe_name};
use crate::engine::Store;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub struct R2 {
    config: Config,
    scratch: PathBuf,
    token: String,
}
impl R2 {
    pub fn new(config: &Config, staging: &Path) -> Result<Self> {
        let output = Command::new("wrangler")
            .args(["auth", "token", "--json"])
            .env_remove("CLOUDFLARE_API_TOKEN")
            .output()?;
        require(output.status.success(), "wrangler authentication failed")?;
        let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let token = response["token"]
            .as_str()
            .ok_or("wrangler did not return a token")?
            .to_string();
        require(
            !token.is_empty()
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)),
            "invalid token encoding",
        )?;
        let scratch = staging.join("scratch");
        fs::create_dir_all(&scratch)?;
        Ok(Self {
            config: config.clone(),
            scratch,
            token,
        })
    }

    fn request(
        &self,
        name: &str,
        body: Option<&[u8]>,
        public: bool,
        immutable: bool,
    ) -> Result<Option<Vec<u8>>> {
        require(safe_name(name), "unsafe remote filename")?;
        let url = if public {
            format!("{}{name}", self.config.public_url)
        } else {
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}/objects/{}{name}",
                self.config.account, self.config.bucket, self.config.prefix
            )
        };
        let destination = self.scratch.join("response");
        let source = self.scratch.join("upload");
        let mut cmd = Command::new("curl");
        cmd.args([
            "--silent",
            "--show-error",
            "--connect-timeout",
            "20",
            "--max-time",
            "180",
            "--proto",
            "=https",
            "--output",
            path_str(&destination)?,
            "--write-out",
            "%{http_code}",
        ]);
        let config = if public {
            String::new()
        } else {
            format!("header = \"Authorization: Bearer {}\"\n", self.token)
        };
        if let Some(bytes) = body {
            fs::write(&source, bytes)?;
            cmd.args([
                "--request",
                "PUT",
                "--data-binary",
                &format!("@{}", path_str(&source)?),
                "--header",
                "Content-Type: application/octet-stream",
                "--header",
                if immutable {
                    "Cache-Control: public, max-age=31536000, immutable"
                } else {
                    "Cache-Control: public, max-age=60, must-revalidate"
                },
            ]);
        }
        // Keep credentials out of argv, files, output and error reports. Never
        // follow redirects from the authenticated origin to another host.
        cmd.args(["--config", "-", "--url", &url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        child
            .stdin
            .take()
            .ok_or("missing curl stdin")?
            .write_all(config.as_bytes())?;
        let response = child.wait_with_output()?;
        require(
            response.status.success(),
            format!("transfer failed: {name}"),
        )?;
        let status = std::str::from_utf8(&response.stdout)?.trim();
        if status == "404" && body.is_none() {
            return Ok(None);
        }
        require(status.starts_with('2'), format!("HTTP {status}: {name}"))?;
        Ok(Some(fs::read(destination)?))
    }
}
impl Store for R2 {
    fn origin(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        self.request(name, None, false, false)
    }
    fn put(&mut self, name: &str, bytes: &[u8], immutable: bool) -> Result<()> {
        println!("Uploading {name}");
        self.request(name, Some(bytes), false, immutable)?;
        Ok(())
    }
    fn public(&mut self, name: &str) -> Result<Vec<u8>> {
        self.request(name, None, true, false)?
            .ok_or_else(|| format!("public 404: {name}; retain staging and retry later").into())
    }
}

const LOCK_REF: &str = "refs/heads/t1bridge-publication-lock";
pub struct GitLock {
    checkout: PathBuf,
    remote: String,
}
impl GitLock {
    pub fn new(config: &Config) -> Self {
        Self {
            checkout: config.checkout.clone().into(),
            remote: format!("git@github.com:{}.git", config.repository),
        }
    }
    pub fn create_owner(&self, plan: &str) -> Result<String> {
        let tree = run("git", &["mktree"], &self.checkout, Some(b""))?;
        let tree = std::str::from_utf8(&tree)?.trim();
        let nonce = fs::read_to_string("/proc/sys/kernel/random/uuid")?;
        let commit = run(
            "git",
            &["commit-tree", tree],
            &self.checkout,
            Some(format!("T1Bridge publication {plan}\nOperation: {}\n", nonce.trim()).as_bytes()),
        )?;
        Ok(std::str::from_utf8(&commit)?.trim().to_string())
    }
    fn current(&self) -> Result<Option<String>> {
        let output = run(
            "git",
            &["ls-remote", "--refs", &self.remote, LOCK_REF],
            &self.checkout,
            None,
        )?;
        Ok(std::str::from_utf8(&output)?
            .split_whitespace()
            .next()
            .map(str::to_string))
    }
    pub fn acquire(&self, owner: &str) -> Result<()> {
        if let Some(current) = self.current()? {
            return require(
                current == owner,
                "another publication owns the remote lock; do not delete it to bypass an active or interrupted publisher",
            );
        }
        run(
            "git",
            &[
                "push",
                "--no-verify",
                &self.remote,
                &format!("--force-with-lease={LOCK_REF}:"),
                &format!("{owner}:{LOCK_REF}"),
            ],
            &self.checkout,
            None,
        )?;
        Ok(())
    }
    pub fn release(&self, owner: &str) -> Result<()> {
        if self.current()?.is_none() {
            return Ok(());
        }
        run(
            "git",
            &[
                "push",
                "--no-verify",
                &self.remote,
                &format!("--force-with-lease={LOCK_REF}:{owner}"),
                &format!(":{LOCK_REF}"),
            ],
            &self.checkout,
            None,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_lock_conflict_owner_resume_and_conditional_release() {
        let nonce = fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap();
        let dir = std::env::temp_dir().join(format!("t1-release-lock-{}", nonce.trim()));
        fs::create_dir(&dir).unwrap();
        run("git", &["init", "--bare", "remote"], &dir, None).unwrap();
        run("git", &["init", "local"], &dir, None).unwrap();
        let local = dir.join("local");
        run(
            "git",
            &["config", "user.name", "Synthetic Publisher"],
            &local,
            None,
        )
        .unwrap();
        run(
            "git",
            &["config", "user.email", "publisher@example.invalid"],
            &local,
            None,
        )
        .unwrap();
        let lock = GitLock {
            checkout: local,
            remote: path_str(&dir.join("remote")).unwrap().to_string(),
        };
        let first = lock.create_owner("first").unwrap();
        let second = lock.create_owner("second").unwrap();
        lock.acquire(&first).unwrap();
        lock.acquire(&first).unwrap();
        assert!(lock.acquire(&second).is_err());
        assert!(lock.release(&second).is_err());
        lock.release(&first).unwrap();
        lock.acquire(&second).unwrap();
        lock.release(&second).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
