//! Talking to a team server about bundles and locks.
//!
//! # Where the credentials come from
//!
//! The agent has no session of its own. The shell holds the token — in Rust and
//! in the OS keychain, never in the webview — and passes it down with the
//! launch that needs it. That keeps one copy of the credential instead of two
//! stores to keep in step, and it means an agent left running after the shell
//! quits cannot start talking to a server on its own behalf.
//!
//! # Why the heartbeat is here and not in the shell
//!
//! The lock says "this profile is open on my machine", and what makes that true
//! is the browser process, which the agent owns. A shell that quits while a
//! profile is still open would stop renewing a lock that is still deserved, and
//! a colleague would take over a profile whose browser is running — the exact
//! data loss the lock exists to prevent.

use std::time::Duration;

/// A server the agent may talk to for the duration of one launch.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Server {
    pub url: String,
    pub token: String,
}

impl Server {
    fn client() -> anyhow::Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            // Generous: a bundle is tens of megabytes and a loaded box is slow,
            // but not unbounded — a wedged server must surface, not hang a
            // launch forever.
            .timeout(Duration::from_secs(300))
            .build()?)
    }

    /// Fetch the current bundle, if the server has one.
    ///
    /// `Ok(None)` for a profile that has never been uploaded — a new profile
    /// shared with a colleague has a row and no bytes, and that is not an error.
    pub async fn fetch_bundle(
        &self,
        profile_id: &str,
    ) -> anyhow::Result<Option<(Vec<u8>, String, i32)>> {
        let res = Self::client()?
            .get(format!("{}/v1/profiles/{profile_id}/bundle", self.url))
            .bearer_auth(&self.token)
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !res.status().is_success() {
            anyhow::bail!("the server refused the bundle ({})", res.status());
        }

        let wrapped = header(&res, "x-fury-wrapped-key")
            .ok_or_else(|| anyhow::anyhow!("the server sent a bundle with no key"))?;
        let version: i32 = header(&res, "x-fury-version")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok(Some((res.bytes().await?.to_vec(), wrapped, version)))
    }

    /// Push a new version, refusing to clobber someone else's.
    ///
    /// The lock token goes with it. The session says who this is; the lock says
    /// this machine is the one running the browser, and only that machine may
    /// write what the browser produced.
    pub async fn push_bundle(
        &self,
        profile_id: &str,
        bytes: &[u8],
        wrapped_key: &str,
        sha256: &str,
        base_version: i32,
        lock_token: &str,
    ) -> anyhow::Result<i32> {
        let res = Self::client()?
            .post(format!("{}/v1/profiles/{profile_id}/bundle", self.url))
            .bearer_auth(&self.token)
            .header("x-fury-sha256", sha256)
            .header("x-fury-wrapped-key", wrapped_key)
            .header("x-fury-base-version", base_version.to_string())
            .header("x-fury-lock-token", lock_token)
            .body(bytes.to_vec())
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::CONFLICT {
            // Said in full rather than as "conflict". The operator's next
            // question is always what happened to their work, and the answer is
            // that it is still on this machine.
            let body: serde_json::Value = res.json().await.unwrap_or_default();
            anyhow::bail!(
                "{} — your work is still here, on this machine, and nothing was overwritten",
                body.get("message").and_then(|v| v.as_str()).unwrap_or("someone uploaded first")
            );
        }
        if !res.status().is_success() {
            anyhow::bail!("the upload was refused ({})", res.status());
        }
        let body: serde_json::Value = res.json().await?;
        Ok(body.get("version").and_then(|v| v.as_i64()).unwrap_or(0) as i32)
    }

    async fn beat(&self, profile_id: &str, lock_token: &str) -> anyhow::Result<()> {
        let res = Self::client()?
            .post(format!("{}/v1/profiles/{profile_id}/lock/heartbeat", self.url))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "lock_token": lock_token }))
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!("heartbeat refused ({})", res.status());
        }
        Ok(())
    }
}

/// The lock lives 90 seconds; renewed every 30.
///
/// Three chances before it lapses. One would mean a single dropped packet hands
/// the profile to whoever asks next while its browser is still running, and a
/// laptop that sleeps for a moment is not a laptop that abandoned the profile.
const BEAT_EVERY: Duration = Duration::from_secs(30);

/// Keep a lock alive until the returned handle is aborted.
pub fn keep_alive(
    server: Server,
    profile_id: String,
    lock_token: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(BEAT_EVERY).await;
            if let Err(e) = server.beat(&profile_id, &lock_token).await {
                // Logged and retried rather than fatal: a lock that lapses is
                // recoverable, and killing a running browser because one
                // request failed is not.
                tracing::warn!(profile = %profile_id, error = %e, "heartbeat failed");
            }
        }
    })
}

fn header(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heartbeat_leaves_room_for_failures() {
        // 90-second lock, renewed every 30: three attempts before it lapses.
        // At 45 a single dropped request would already be half the budget.
        assert!(BEAT_EVERY.as_secs() * 3 <= 90);
    }

    #[test]
    fn a_server_needs_both_halves() {
        let parsed: Result<Server, _> =
            serde_json::from_value(serde_json::json!({ "url": "https://x" }));
        // A token-less server would send anonymous requests and be reported to
        // the operator as an expired session.
        assert!(parsed.is_err());
    }
}
