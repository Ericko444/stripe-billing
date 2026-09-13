use audit::{Action, Actor, CorrelationId, Target, TargetId};
use identity_domain::{
    AccountEvent, Clock, Email, MailPurpose, Mailer, NewPassword, NewPasswordToken, OutgoingMail,
    Password, PasswordHasher, PasswordPolicyError, PasswordTokenRepository, SplitToken,
    TokenPurpose, UserId, UserRepository,
};
use thiserror::Error;
use time::{Duration, OffsetDateTime};

use crate::token::{generate_token, password_link};

/// How long a reset link works: short, because it is a bearer credential
/// travelling through a mailbox. Long enough to switch to a mail client and
/// back; not long enough to sit usable in an inbox for a day.
pub const RESET_TOKEN_LIFETIME: Duration = Duration::minutes(15);

/// How long an invitation link works: longer, because an invitee is not
/// waiting at a keyboard for it -- someone else added them, at a moment of
/// that person's choosing. Still single use, and it can only ever set a
/// *first* password.
pub const INVITATION_TOKEN_LIFETIME: Duration = Duration::hours(72);

/// Password reset: issuing links (off the request path, in the worker) and
/// completing them.
///
/// Nothing in `issue` runs while a caller waits. The HTTP handler only
/// enqueues the address; `issue` runs later, in the reset worker. That is
/// what makes the request's response -- body *and* timing -- independent of
/// whether the address has an account: the work that depends on it does not
/// happen until the response has been sent.
pub struct PasswordResetService<U, T, H, M, C> {
    users: U,
    tokens: T,
    hasher: H,
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

/// Why a reset link was not honoured.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompleteResetError {
    /// The new password breaks the policy. Checked before the link is even
    /// looked at, so a rejected password never uses up the link.
    #[error("new password rejected: {0}")]
    Policy(PasswordPolicyError),
    /// Malformed, unknown, forged, expired, superseded or already used --
    /// **one variant for all of them**, so the answer says nothing about
    /// which, or about whether the link was ever real.
    #[error("invalid or expired link")]
    InvalidLink,
    /// A dependency failed. The message is for logs.
    #[error("password reset unavailable: {0}")]
    Unavailable(String),
}

fn unavailable(err: &dyn std::fmt::Display) -> String {
    err.to_string()
}

impl<U, T, H, M, C> PasswordResetService<U, T, H, M, C>
where
    U: UserRepository,
    T: PasswordTokenRepository,
    H: PasswordHasher,
    M: Mailer,
    C: Clock,
{
    /// A service mailing links under `link_base` -- the frontend's origin,
    /// e.g. `http://localhost:5173`.
    pub fn new(
        users: U,
        tokens: T,
        hasher: H,
        mailer: M,
        clock: C,
        link_base: impl Into<String>,
    ) -> Self {
        Self {
            users,
            tokens,
            hasher,
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
        let Some(user) = self
            .users
            .find_by_email(email)
            .await
            .map_err(|err| ResetError::Unavailable(unavailable(&err)))?
            .filter(|user| user.is_active())
        else {
            return Ok(IssueOutcome::NoActiveAccount);
        };

        let token = generate_token().map_err(|err| ResetError::Unavailable(unavailable(&err)))?;
        let now = self.clock.now();
        let stored = NewPasswordToken {
            selector: token.selector(),
            verifier_hash: token.verifier().hash(),
            user_id: user.id,
            purpose: TokenPurpose::PasswordReset,
            created_at: now,
            expires_at: now + RESET_TOKEN_LIFETIME,
        };
        let event = anonymous(user.id, Action::PasswordResetRequested, now, correlation_id);
        self.tokens
            .replace(&stored, Some(&event))
            .await
            .map_err(|err| ResetError::Unavailable(unavailable(&err)))?;

        let mail = OutgoingMail {
            to: user.email,
            purpose: MailPurpose::PasswordReset,
            link: password_link(&self.link_base, &token),
            correlation_id,
        };
        self.mailer
            .send(&mail)
            .await
            .map_err(|err| ResetError::Unavailable(unavailable(&err)))?;
        Ok(IssueOutcome::Issued)
    }

    /// Sets a password through a link: a reset link, or an invitation link --
    /// one route and one page serve both.
    ///
    /// In order, cheapest refusal first:
    ///
    /// 1. the new password against the policy -- a refusal here never touches
    ///    the link, which stays usable;
    /// 2. the link's form, its selector's row, its verifier (constant time)
    ///    and its expiry by the module's clock -- one error for every
    ///    failure, and no Argon2 cost for a link that was never going to work;
    /// 3. the Argon2id hash of the new password;
    /// 4. one transaction that re-checks the link under a row lock, consumes
    ///    it, stores the hash, **deletes every session of the user in every
    ///    tenant**, deletes every other outstanding link, and records, once
    ///    per tenant, `PasswordResetCompleted` and `SessionsRevoked` for a
    ///    reset or `InvitationAccepted` for an invitation. An invitation is
    ///    refused there if the account has a password by then.
    ///
    /// No session is issued. Holding a link proves access to a mailbox, not
    /// the new password; the user logs in with it, like anyone else.
    pub async fn complete(
        &self,
        presented: &str,
        new: Password,
        correlation_id: CorrelationId,
    ) -> Result<(), CompleteResetError> {
        let new = NewPassword::check(new).map_err(CompleteResetError::Policy)?;

        let token = SplitToken::parse(presented).map_err(|_| CompleteResetError::InvalidLink)?;
        let stored = self
            .tokens
            .find(token.selector())
            .await
            .map_err(|err| CompleteResetError::Unavailable(unavailable(&err)))?
            .ok_or(CompleteResetError::InvalidLink)?;
        let now = self.clock.now();
        if !stored.verifier_hash.verifies(token.verifier()) || stored.expires_at <= now {
            return Err(CompleteResetError::InvalidLink);
        }

        let hash = self
            .hasher
            .hash(&new)
            .await
            .map_err(|err| CompleteResetError::Unavailable(unavailable(&err)))?;
        let event = |action| anonymous(stored.user_id, action, now, correlation_id);
        let events = match stored.purpose {
            TokenPurpose::PasswordReset => vec![
                event(Action::PasswordResetCompleted),
                event(Action::SessionsRevoked),
            ],
            // No `SessionsRevoked`: an account without a password has never
            // had a session to revoke.
            TokenPurpose::Invitation => vec![event(Action::InvitationAccepted)],
        };
        let completed = self
            .tokens
            .redeem(
                token.selector(),
                stored.purpose,
                &stored.verifier_hash,
                now,
                &hash,
                &events,
            )
            .await
            .map_err(|err| CompleteResetError::Unavailable(unavailable(&err)))?;
        if !completed {
            // Used, superseded or expired between the check above and the
            // row lock -- a concurrent completion won -- or an invitation to
            // an account that has had a password set since.
            return Err(CompleteResetError::InvalidLink);
        }
        Ok(())
    }
}

/// An account event with no authenticated actor behind it: whoever holds the
/// mailbox or the link.
fn anonymous(
    user: UserId,
    action: Action,
    occurred_at: OffsetDateTime,
    correlation_id: CorrelationId,
) -> AccountEvent {
    AccountEvent {
        actor: Actor::Anonymous,
        action,
        target: Target::User(TargetId::new(user.as_uuid())),
        occurred_at,
        correlation_id,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{PasswordHash, User};
    use secrecy::ExposeSecret;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeHasher, FakeMailer, FakeTokens, FakeUsers, FixedClock};

    type Service = PasswordResetService<FakeUsers, FakeTokens, FakeHasher, FakeMailer, FixedClock>;

    const NEW_PASSWORD: &str = "a much longer and newer passphrase";

    struct World {
        users: FakeUsers,
        tokens: FakeTokens,
        mailer: FakeMailer,
        clock: FixedClock,
        alice: User,
    }

    impl World {
        fn service_at(&self, clock: FixedClock) -> Service {
            PasswordResetService::new(
                self.users.clone(),
                self.tokens.clone(),
                FakeHasher::default(),
                self.mailer.clone(),
                clock,
                "http://localhost:5173/",
            )
        }

        fn service(&self) -> Service {
            self.service_at(self.clock)
        }

        /// Issues a link for Alice and returns its token, as mailed.
        async fn mailed_token(&self) -> Result<String, Box<dyn Error>> {
            self.service()
                .issue(&self.alice.email, CorrelationId::new(Uuid::new_v4()))
                .await?;
            let sent = self.mailer.sent();
            let Some((_, _, link, _)) = sent.last() else {
                return Err("no mail was sent".into());
            };
            Ok(link
                .split_once("#token=")
                .map(|(_, token)| token.to_string())
                .unwrap_or_default())
        }
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

        Ok(World {
            users: FakeUsers::with(vec![alice.clone(), dave]),
            tokens: FakeTokens::default(),
            mailer,
            clock: FixedClock::at_epoch_plus_days(20_000),
            alice,
        })
    }

    fn password(raw: &str) -> Password {
        Password::new(raw.to_string())
    }

    #[tokio::test]
    async fn an_active_account_gets_a_fifteen_minute_token_a_mail_and_an_audit_event()
    -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        let outcome = w.service().issue(&w.alice.email, correlation_id).await?;

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
        assert_eq!(*purpose, MailPurpose::PasswordReset);
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
                .service()
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
            .service()
            .issue(&w.alice.email, CorrelationId::new(Uuid::new_v4()))
            .await;

        assert!(matches!(result, Err(ResetError::Unavailable(_))));
        assert_eq!(w.tokens.replaced().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn completing_stores_the_new_hash_and_records_completion_and_revocation()
    -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let wire = w.mailed_token().await?;
        let correlation_id = CorrelationId::new(Uuid::new_v4());

        w.service()
            .complete(&wire, password(NEW_PASSWORD), correlation_id)
            .await?;

        let completed = w.tokens.completed();
        let [(user, hash, events)] = completed.as_slice() else {
            return Err("expected one completion".into());
        };
        assert_eq!(*user, w.alice.id);
        assert_eq!(hash, &FakeHasher::hash_of(NEW_PASSWORD));
        let actions: Vec<(Actor, Action)> = events.iter().map(|e| (e.actor, e.action)).collect();
        assert_eq!(
            actions,
            vec![
                (Actor::Anonymous, Action::PasswordResetCompleted),
                (Actor::Anonymous, Action::SessionsRevoked),
            ]
        );
        assert!(events.iter().all(|e| e.correlation_id == correlation_id));
        Ok(())
    }

    /// R5 -- a reset link works for fifteen minutes by the module's clock,
    /// and not one second more.
    #[tokio::test]
    async fn a_reset_token_is_refused_after_fifteen_minutes() -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let wire = w.mailed_token().await?;
        let issued = w.clock.now();

        let at_expiry = w.service_at(FixedClock::at(issued + Duration::minutes(15)));
        let result = at_expiry
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;
        assert_eq!(result.err(), Some(CompleteResetError::InvalidLink));

        let just_before = w.service_at(FixedClock::at(
            issued + Duration::minutes(15) - Duration::seconds(1),
        ));
        just_before
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn an_invitation_link_is_redeemed_on_the_same_route_and_audited_as_accepted()
    -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let token = SplitToken::from_bytes([5; 16], [5; 32]);
        let issued = w.clock.now();
        w.tokens
            .replace(
                &NewPasswordToken {
                    selector: token.selector(),
                    verifier_hash: token.verifier().hash(),
                    user_id: w.alice.id,
                    purpose: TokenPurpose::Invitation,
                    created_at: issued,
                    expires_at: issued + INVITATION_TOKEN_LIFETIME,
                },
                None,
            )
            .await?;
        let wire = token.to_wire().expose_secret().to_string();

        // Still good after a day -- where a reset link would long be dead.
        w.service_at(FixedClock::at(issued + Duration::hours(24)))
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;

        let completed = w.tokens.completed();
        let [(user, _, events)] = completed.as_slice() else {
            return Err("expected one completion".into());
        };
        assert_eq!(*user, w.alice.id);
        let actions: Vec<(Actor, Action)> = events.iter().map(|e| (e.actor, e.action)).collect();
        assert_eq!(
            actions,
            vec![(Actor::Anonymous, Action::InvitationAccepted)]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_policy_failure_leaves_the_link_usable() -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let wire = w.mailed_token().await?;
        let token = SplitToken::parse(&wire)?;

        let rejected = w
            .service()
            .complete(&wire, password("short"), CorrelationId::new(Uuid::new_v4()))
            .await;

        assert_eq!(
            rejected.err(),
            Some(CompleteResetError::Policy(PasswordPolicyError::TooShort))
        );
        assert!(w.tokens.is_outstanding(token.selector()));
        w.service()
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn every_bad_link_is_the_same_error() -> Result<(), Box<dyn Error>> {
        let w = world(FakeMailer::default())?;
        let wire = w.mailed_token().await?;
        let token = SplitToken::parse(&wire)?;
        let forged = SplitToken::from_bytes(*token.selector().as_bytes(), [0; 32])
            .to_wire()
            .expose_secret()
            .to_string();
        let unknown = SplitToken::from_bytes([9; 16], [9; 32])
            .to_wire()
            .expose_secret()
            .to_string();

        for presented in ["", "not-a-token", forged.as_str(), unknown.as_str()] {
            let result = w
                .service()
                .complete(
                    presented,
                    password(NEW_PASSWORD),
                    CorrelationId::new(Uuid::new_v4()),
                )
                .await;
            assert_eq!(
                result.err(),
                Some(CompleteResetError::InvalidLink),
                "{presented:?}"
            );
        }

        // And a link that worked once is, the second time, the same error.
        w.service()
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await?;
        let reused = w
            .service()
            .complete(
                &wire,
                password(NEW_PASSWORD),
                CorrelationId::new(Uuid::new_v4()),
            )
            .await;
        assert_eq!(reused.err(), Some(CompleteResetError::InvalidLink));
        Ok(())
    }
}
