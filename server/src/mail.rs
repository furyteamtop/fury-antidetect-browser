// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Email, for one job: proving that whoever redeems an invitation reads the
//! inbox it was issued for.
//!
//! An invitation code is a bearer credential. Pasted into a group chat, it
//! enrolled whoever typed it first under the invited address, with their own
//! password, and in Users they looked like the newcomer waiting to be let in
//! (the owner, 14.10.2026). With mail configured, redeeming a code also takes
//! six digits sent to that address (api.rs, enroll_email_*).
//!
//! Off unless `FURY_SMTP_URL` is set. A self-hosted server with no mail keeps
//! working exactly as before, and the invitation screen says the code is then
//! the only thing standing between the address and a stranger.
//!
//! ```text
//! FURY_SMTP_URL=smtps://user:password@smtp.example.com:465
//! FURY_SMTP_URL=smtp://user:password@smtp.example.com:587?tls=required
//! FURY_MAIL_FROM=Fury <noreply@example.com>
//! ```

use lettre::message::{header::ContentType, Mailbox};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    /// `None` when mail is not configured. A setting that is present but
    /// wrong is an error at start-up, not at the first invitation: an owner
    /// who set up mail should not find out it never worked from a newcomer
    /// who got no letter.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(url) = std::env::var("FURY_SMTP_URL").ok().filter(|v| !v.trim().is_empty()) else {
            return Ok(None);
        };
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url.trim())
            .map_err(|e| anyhow::anyhow!("FURY_SMTP_URL: {e}"))?
            .build();
        let from = std::env::var("FURY_MAIL_FROM")
            .map_err(|_| anyhow::anyhow!("FURY_SMTP_URL is set but FURY_MAIL_FROM is not: letters need a sender"))?
            .parse::<Mailbox>()
            .map_err(|e| anyhow::anyhow!("FURY_MAIL_FROM: {e}"))?;
        Ok(Some(Self { transport, from }))
    }

    /// Whether the SMTP server answers and takes our login. Logged at start-up
    /// rather than enforced: a mail host down for a minute should not keep the
    /// whole server from starting.
    pub async fn check(&self) -> anyhow::Result<()> {
        match self.transport.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => anyhow::bail!("the SMTP server did not answer"),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn send(&self, to: &str, subject: &str, body: String) -> anyhow::Result<()> {
        let message = Message::builder()
            .from(self.from.clone())
            .to(to.parse::<Mailbox>()?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;
        self.transport.send(message).await?;
        Ok(())
    }
}

/// The letter with the six digits, in the language the newcomer's app is in.
/// Plain text: it carries one number, and HTML mail is where phishing lives.
/// The second step of an account recovery (recovery.rs), when the account has
/// no authenticator app and this server sends mail.
pub fn recovery_code_letter(lang: &str, code: &str, minutes: i64) -> (String, String) {
    if lang.starts_with("ru") {
        (
            format!("Восстановление доступа к Fury: {code}"),
            format!(
                "Кто-то ввёл ключ восстановления вашего аккаунта Fury. Код, чтобы продолжить: {code}\n\n\
                 Он действует {minutes} минут.\n\n\
                 Если это не вы, ваш ключ восстановления больше не тайна: войдите в Fury и создайте \
                 новый в настройках, старый перестанет действовать. Без этого кода никто не сменит пароль.\n"
            ),
        )
    } else {
        (
            format!("Fury account recovery: {code}"),
            format!(
                "Somebody entered the recovery key of your Fury account. The code to continue: {code}\n\n\
                 It is valid for {minutes} minutes.\n\n\
                 If it was not you, your recovery key is no longer a secret: sign in to Fury and make a \
                 new one in Settings, which ends the old one. Without this code nobody can change your password.\n"
            ),
        )
    }
}

pub fn email_code_letter(lang: &str, org: &str, code: &str, minutes: i64) -> (String, String) {
    if lang.starts_with("ru") {
        (
            format!("Код подтверждения Fury: {code}"),
            format!(
                "Код для регистрации в команде «{org}» в Fury: {code}\n\n\
                 Введите его в окне регистрации. Код действует {minutes} минут.\n\n\
                 Если вы не регистрировались в Fury, просто удалите это письмо: \
                 без этого кода аккаунт на ваш адрес никто не создаст.\n"
            ),
        )
    } else {
        (
            format!("Fury confirmation code: {code}"),
            format!(
                "Your code for joining the team \"{org}\" in Fury: {code}\n\n\
                 Type it into the sign-up window. It is valid for {minutes} minutes.\n\n\
                 If you did not sign up for Fury, delete this letter: without the \
                 code nobody can make an account under your address.\n"
            ),
        )
    }
}
