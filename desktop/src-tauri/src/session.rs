//! The session token, and the two places it may live.
//!
//! In the browser build the token sat in `localStorage`. The argument against
//! that is not the obvious one: the interface renders every server-supplied
//! string — profile names, proxy hostnames, a colleague's email — through JSX,
//! which escapes it, and there is no `innerHTML` anywhere in `../src`. So there
//! is no injection sink to worry about today.
//!
//! The sharp edge is different. Sessions slide forward on every authenticated
//! request (`server/src/auth.rs`), so a token that leaves this machine once
//! never expires as long as something touches the server twice a day. A
//! credential with that property should not sit in a store that backup
//! software, sync clients and crash reporters copy by default. Keeping it in
//! Rust turns "steal a credential that outlives you" into "abuse the app while
//! it happens to be running" — narrower, and the narrowing is the point.
//!
//! What this is *not*: protection from another process running as the same
//! user. On macOS keychain items carry a per-item ACL bound to the requesting
//! binary's signature, which is a real boundary — but only once this app has a
//! stable code-signing identity, which it does not yet. On Windows, Credential
//! Manager blobs are DPAPI-encrypted under the user's key with no
//! per-application ACL, so any process running as that user can read them. The
//! honest summary is: better than a file on disk, not a sandbox.

use std::sync::Mutex;

const SERVICE: &str = "dev.fury.desktop";

/// Holds the token for the current process and mirrors it into the keychain.
pub struct Session {
    /// Keyed by server URL, so pointing the app at a second server does not
    /// resurrect a token issued by the first.
    account: Mutex<String>,
    token: Mutex<Option<String>>,
    /// Whether the keychain has been consulted for the current account.
    ///
    /// The read is lazy, and that is not an optimisation. macOS ties keychain
    /// permission to the signature of the binary asking, so a rebuilt
    /// application — every development build, and every user the day after an
    /// update — gets a dialog, and `SecKeychainFindGenericPassword` blocks
    /// until it is answered. Doing that in `setup()` blocked the main thread
    /// before the window existed: an application that started, showed nothing,
    /// and waited for a dialog that had no window to appear over. It looked
    /// exactly like a hang, and it was one.
    ///
    /// The same mistake, in the same week, in the agent. Twice is a rule: no
    /// keychain read on a thread that something is waiting to draw on.
    loaded: Mutex<bool>,
    /// A read already in flight. Without this, every caller that timed out
    /// started another one, and each started another dialog — so answering the
    /// first left a queue of identical prompts behind it.
    reading: Mutex<bool>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            account: Mutex::new(String::new()),
            token: Mutex::new(None),
            loaded: Mutex::new(false),
            reading: Mutex::new(false),
        }
    }

    /// Points the session at a server. Does not touch the keychain — the stored
    /// token is fetched the first time something asks for it.
    pub fn bind(&self, server_url: &str) {
        *self.account.lock().unwrap() = server_url.to_string();
        *self.token.lock().unwrap() = None;
        *self.loaded.lock().unwrap() = false;
    }

    /// Read the stored token once per account.
    fn ensure_loaded(&self) {
        {
            if *self.loaded.lock().unwrap() {
                return;
            }
        }
        let account = self.account.lock().unwrap().clone();
        if account.is_empty() {
            return;
        }
        // On its own thread, with a bound. If the keychain puts a dialog up,
        // this waits for a person — and a person may be at lunch. Failing to
        // read a stored token means "not signed in", which is recoverable by
        // signing in; blocking the interface until someone clicks is not
        // recoverable by anything.
        //
        // Not marked loaded on a timeout, so the next call tries again — by
        // which time the dialog has usually been answered.
        {
            let mut reading = self.reading.lock().unwrap();
            if *reading {
                // Someone is already waiting on the prompt. Answer as "not
                // signed in" rather than queueing a second one behind it.
                return;
            }
            *reading = true;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let for_thread = account.clone();
        std::thread::spawn(move || {
            let stored = entry(&for_thread).and_then(|e| match e.get_password() {
                Ok(t) => Some(t),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => {
                    eprintln!("session: keychain read failed: {e}");
                    None
                }
            });
            let _ = tx.send(stored);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(stored) => {
                *self.token.lock().unwrap() = stored;
                *self.loaded.lock().unwrap() = true;
                *self.reading.lock().unwrap() = false;
            }
            Err(_) => {
                eprintln!(
                    "session: the keychain has not answered — it is probably asking permission. \
                     Carrying on as signed out; this will resolve once the prompt is answered."
                );
            }
        }
    }

    pub fn token(&self) -> Option<String> {
        self.ensure_loaded();
        self.token.lock().unwrap().clone()
    }

    pub fn is_signed_in(&self) -> bool {
        self.ensure_loaded();
        self.token.lock().unwrap().is_some()
    }

    pub fn store(&self, token: &str) {
        *self.token.lock().unwrap() = Some(token.to_string());
        *self.loaded.lock().unwrap() = true;
        let account = self.account.lock().unwrap().clone();
        if let Some(e) = entry(&account) {
            if let Err(err) = e.set_password(token) {
                // Not fatal. A keychain that refuses to write costs the operator
                // a sign-in next launch; refusing to run would cost them the
                // session they just started.
                eprintln!("session: could not persist to keychain: {err}");
            }
        }
    }

    /// Forgets the token locally. Revoking it server-side is a separate call and
    /// must happen first — see the `logout` command.
    pub fn clear(&self) {
        *self.token.lock().unwrap() = None;
        *self.loaded.lock().unwrap() = true;
        let account = self.account.lock().unwrap().clone();
        if let Some(e) = entry(&account) {
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => eprintln!("session: could not clear keychain: {err}"),
            }
        }
    }
}

fn entry(account: &str) -> Option<keyring::Entry> {
    if account.is_empty() {
        return None;
    }
    keyring::Entry::new(SERVICE, account)
        .inspect_err(|e| eprintln!("session: no credential store available: {e}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_session_holds_nothing() {
        let s = Session::new();
        assert!(!s.is_signed_in());
        assert_eq!(s.token(), None);
    }

    #[test]
    fn an_unbound_session_does_not_touch_the_keychain() {
        // bind() has not been called, so there is no account to key on. Storing
        // must not panic and must not write under an empty account name, which
        // would collide across every server the app is ever pointed at.
        let s = Session::new();
        s.store("token-value");
        assert_eq!(s.token().as_deref(), Some("token-value"));
        assert!(entry("").is_none());
    }

    #[test]
    fn clearing_removes_the_in_memory_copy() {
        let s = Session::new();
        s.store("token-value");
        s.clear();
        assert!(!s.is_signed_in());
    }
}
