//! The demo's `Mailer`: it writes the mail to the log instead of sending it.
//!
//! **The one place in the system where a reset or invitation link reaches a
//! log line.** That contradicts the rule the rest of the identity module
//! keeps -- no token in a log -- and does so on purpose: without it, nobody
//! running the demo could follow a reset link. It is confined to `demo`,
//! `main` warns at startup that it is in use, and a real `Mailer` hands the
//! body to a mail provider and logs, at most, that a mail was sent.

use identity_domain::{MailError, Mailer, OutgoingMail, TokenPurpose};
use secrecy::ExposeSecret;

/// Logs each mail, link included, at `warn` so it stands out.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogMailer;

impl Mailer for LogMailer {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError> {
        let subject = match mail.purpose {
            TokenPurpose::PasswordReset => "Reset your password",
            TokenPurpose::Invitation => "You have been invited -- choose a password",
        };
        tracing::warn!(
            correlation_id = %mail.correlation_id.as_uuid(),
            to = %mail.to.as_str(),
            subject,
            link = %mail.link.expose_secret(),
            "DEMO MAILER: not sent, logged instead"
        );
        Ok(())
    }
}
