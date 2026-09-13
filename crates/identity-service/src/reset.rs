use audit::{Action, Actor, CorrelationId, Target, TargetId};
use identity_domain::{
    AccountEvent, Clock, Email, Mailer, NewPasswordToken, OutgoingMail, PasswordTokenRepository,
    SplitToken, TokenPurpose, UserRepository,
};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use time::Duration;

use crate::token::generate_token;

/// How long a reset link works: short, because it is a bearer credential
/// travelling through a mailbox. Long enough to switch to a mail client and
/// back; not long enough to sit usable in an inbox for a day.
pub const RESET_TOKEN_LIFETIME: Duration = Duration::minutes(15);

/// Issues password reset links, off the request path.
///
/// Nothing here runs while a caller waits. The HTTP handler only enqueues
/// the address; this service runs later, in the reset worker. That is what
/// makes the request's response -- body *and* timing -- independent of
/// whether the address has an account: the work that depends on it does not
/// happen until the response has been sent.
pub struct PasswordResetService<U, T, M, C> {
    users: U,
    tokens: T,
    mailer: M,
    clock: C,
    link_base: String,
}

/// What issuing did, for the worker's log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueOutcome {
    /// A link was stored and mailed.
    Issued,
    /// The address has no active account; nothing was stored or sent.
    NoActiveAccount,
}

/// Issuing could not complete.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResetError {
    /// A dependency failed. The message is for logs.
    #[error("password reset unavailable: {0}")]
    Unavailable(String),
}

impl<U, T, M, C> PasswordResetService<U, T, M, C>
where
    U: UserRepository,
    T: PasswordTokenRepository,
    M: Mailer,
    C: Clock,
{
    /// A service mailing links under `link_base` -- the frontend's origin,
    /// e.g. `http://localhost:5173`.
    pub fn new(users: U, tokens: T, mailer: M, clock: C, link_base: impl Into<String>) -> Self {
        Self {
            users,
            tokens,
            mailer,
            clock,
            link_base: link_base.into(),
        }
    }

    /// Issues and mails a reset link for `email`, if it belongs to an active
    /// account.
    ///
    /// The new token replaces any outstanding one in the same transaction as
    /// the `PasswordResetRequested` audit rows, and only then is the mail
    /// sent -- a mail that fails leaves a valid link nobody received, which
    /// the user recovers from by asking again, rather than a mail pointing at
    /// a link that was never stored.
    pub async fn issue(
        &self,
        email: &Email,
        correlation_id: CorrelationId,
    ) -> Result<IssueOutcome, ResetError> {
        let unavailable = |err: &dyn std::fmt::Display| ResetError::Unavailable(err.to_string());

        let Some(user) = self
            .users
            .find_by_email(email)
            .await
            .map_err(|err| unavailable(&err))?
            .filter(|user| user.is_active())
        else {
            return Ok(IssueOutcome::NoActiveAccount);
        };

        let token = generate_token().map_err(|err| unavailable(&err))?;
        let now = self.clock.now();
        let stored = NewPasswordToken {
            selector: token.selector(),
            verifier_hash: token.verifier().hash(),
            user_id: user.id,
            purpose: TokenPurpose::PasswordReset,
            created_at: now,
            expires_at: now + RESET_TOKEN_LIFETIME,
        };
        let event = AccountEvent {
            actor: Actor::Anonymous,
            action: Action::PasswordResetRequested,
            target: Target::User(TargetId::new(user.id.as_uuid())),
            occurred_at: now,
            correlation_id,
        };
        self.tokens
            .replace(&stored, Some(&event))
            .await
            .map_err(|err| unavailable(&err))?;

        let mail = OutgoingMail {
            to: user.email,
            purpose: TokenPurpose::PasswordReset,
            link: self.link_for(&token),
            correlation_id,
        };
        self.mailer
            .send(&mail)
            .await
            .map_err(|err| unavailable(&err))?;
        Ok(IssueOutcome::Issued)
    }

    /// `{base}/reset-password#token={token}`.
    ///
    /// The token rides in the **fragment**. A fragment is never sent to a
    /// server, so the credential stays out of access logs, proxy logs and the
    /// `Referer` of anything the reset page loads; the page reads it with
    /// script and sends it in a request body.
    fn link_for(&self, token: &SplitToken) -> SecretString {
        SecretString::from(format!(
            "{}/reset-password#token={}",
            self.link_base.trim_end_matches('/'),
            token.to_wire().expose_secret()
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{PasswordHash, User, UserId};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeMailer, FakeTokens, FakeUsers, FixedClock};

    type Service = PasswordResetService<FakeUsers, FakeTokens, FakeMailer, FixedClock>;

    struct World {
        service: Service,
        tokens: FakeTokens,
        mailer: FakeMailer,
        clock: FixedClock,
        alice: User,
    }

    fn world(mailer: FakeMailer) -> Result<World, Box<dyn Error>> {
        let alice = User {
            id: UserId::new(Uuid::new_v4()),
            email: Email::parse("alice@example.test")?,
            display_name: String::new(),
            password_hash: Some(PasswordHash::new("fake:old".into())),
            deactivated_at: None,
        };
        let mut dave = alice.clone();
        dave.id = UserId::new(Uuid::new_v4());
        dave.email = Email::parse("dave@example.test")?;
        dave.deactivated_at = Some(OffsetDateTime::UNIX_EPOCH);

        let tokens = FakeTokens::default();
        let clock = FixedClock::at_epoch_plus_days(20_000);
        Ok(World {
            service: PasswordResetService::new(
                FakeUsers::with(vec![alice.clone(), dave]),
                tokens.clone(),
                mailer.clone(),
                clock,
                "http://localhost:5173/",
            ),
            tokens,
            mailer,
            clock,
            alice,
        })
    }

    #[tokio::test]
    async fn an_active_account_gets_a_fifteen_minute_token_a_mail_and_an_audit_event()
    -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        let outcome = w.service.issue(&w.alice.email, correlation_id).await?;

        assert_eq!(outcome, IssueOutcome::Issued);
        let replaced = w.tokens.replaced();
        let [(stored, Some(event))] = replaced.as_slice() else {
            return Err("expected one audited token".into());
        };
        assert_eq!(stored.user_id, w.alice.id);
        assert_eq!(stored.purpose, TokenPurpose::PasswordReset);
        assert_eq!(stored.created_at, w.clock.now());
        assert_eq!(stored.expires_at, w.clock.now() + Duration::minutes(15));
        assert_eq!(event.actor, Actor::Anonymous);
        assert_eq!(event.action, Action::PasswordResetRequested);
        assert_eq!(event.correlation_id, correlation_id);

        let mails = w.mailer.sent();
        let [(to, purpose, link, mail_correlation)] = mails.as_slice() else {
            return Err("expected one mail".into());
        };
        assert_eq!(to, &w.alice.email);
        assert_eq!(*purpose, TokenPurpose::PasswordReset);
        assert_eq!(*mail_correlation, correlation_id);

        // The mailed link carries the stored token -- in the fragment.
        let Some(wire) = link.strip_prefix("http://localhost:5173/reset-password#token=") else {
            return Err(format!("unexpected link shape: {link}").into());
        };
        let mailed = SplitToken::parse(wire)?;
        assert_eq!(mailed.selector(), stored.selector);
        assert!(stored.verifier_hash.verifies(mailed.verifier()));
        Ok(())
    }

    #[tokio::test]
    async fn unknown_and_deactivated_addresses_get_nothing() -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;

        for address in ["nobody@example.test", "dave@example.test"] {
            let outcome = w
                .service
                .issue(&Email::parse(address)?, CorrelationId::new(Uuid::new_v4()))
                .await?;
            assert_eq!(outcome, IssueOutcome::NoActiveAccount, "{address}");
        }
        assert!(w.tokens.replaced().is_empty());
        assert!(w.mailer.sent().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_mail_failure_is_reported_after_the_token_is_stored() -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::failing())?;

        let result = w
            .service
            .issue(&w.alice.email, CorrelationId::new(Uuid::new_v4()))
            .await;

        assert!(matches!(result, Err(ResetError::Unavailable(_))));
        assert_eq!(w.tokens.replaced().len(), 1);
        Ok(())
    }
}
