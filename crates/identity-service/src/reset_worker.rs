use std::sync::Arc;

use audit::CorrelationId;
use identity_domain::{
    Clock, Email, Mailer, PasswordHasher, PasswordTokenRepository, UserRepository,
};
use tokio::sync::mpsc;

use crate::{IssueOutcome, PasswordResetService};

/// How many reset requests may wait for the worker. Past this, requests are
/// dropped -- the requester still gets the same answer, and asks again.
pub const RESET_QUEUE_CAPACITY: usize = 1024;

/// One reset request, as the HTTP handler hands it over: the address and the
/// request it came from, so the worker's audit rows and log lines carry the
/// same correlation id as the response the requester already has.
#[derive(Debug)]
pub struct ResetJob {
    /// The address that was submitted, normalised.
    pub email: Email,
    /// The request that submitted it.
    pub correlation_id: CorrelationId,
}

/// The sending half, held by the HTTP layer.
#[derive(Debug, Clone)]
pub struct ResetQueue(mpsc::Sender<ResetJob>);

/// The receiving half, handed to [`run_reset_worker`].
#[derive(Debug)]
pub struct ResetReceiver(mpsc::Receiver<ResetJob>);

/// A bounded queue between the reset request handler and the worker.
pub fn reset_queue(capacity: usize) -> (ResetQueue, ResetReceiver) {
    let (sender, receiver) = mpsc::channel(capacity);
    (ResetQueue(sender), ResetReceiver(receiver))
}

impl ResetReceiver {
    /// Takes one waiting job without waiting, if there is one. For tests that
    /// assert what a request queued without running the worker.
    pub fn try_recv(&mut self) -> Option<ResetJob> {
        self.0.try_recv().ok()
    }
}

impl ResetQueue {
    /// Queues `job` without waiting. `false` if the queue is full or the
    /// worker has stopped -- the caller answers the request the same way
    /// regardless, and logs it.
    pub fn enqueue(&self, job: ResetJob) -> bool {
        self.0.try_send(job).is_ok()
    }
}

/// Issues a reset link for each queued request until every [`ResetQueue`]
/// is dropped.
///
/// A failure is logged and the loop carries on: one unreachable mail server
/// must not stop the next request. The in-process queue does not survive a
/// restart -- a lost request is recovered by the user asking again, and a
/// deployment that needs better keeps an outbox table instead.
pub async fn run_reset_worker<U, T, H, M, C>(
    mut receiver: ResetReceiver,
    service: Arc<PasswordResetService<U, T, H, M, C>>,
) where
    U: UserRepository,
    T: PasswordTokenRepository,
    H: PasswordHasher,
    M: Mailer,
    C: Clock,
{
    while let Some(job) = receiver.0.recv().await {
        let correlation_id = job.correlation_id.as_uuid();
        match service.issue(&job.email, job.correlation_id).await {
            Ok(IssueOutcome::Issued) => {
                tracing::info!(%correlation_id, "password reset link issued");
            }
            Ok(IssueOutcome::NoActiveAccount) => {
                // The fingerprint, never the address: this log line must be
                // erasable in the same way the audit journal is.
                tracing::info!(
                    %correlation_id,
                    address = %job.email.fingerprint(),
                    "password reset requested for an address with no active account"
                );
            }
            Err(err) => {
                tracing::error!(%correlation_id, error = %err, "password reset failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use identity_domain::{PasswordHash, User, UserId};
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{FakeHasher, FakeMailer, FakeTokens, FakeUsers, FixedClock};

    fn job(address: &str) -> Result<ResetJob, Box<dyn Error>> {
        Ok(ResetJob {
            email: Email::parse(address)?,
            correlation_id: CorrelationId::new(Uuid::new_v4()),
        })
    }

    #[tokio::test]
    async fn the_worker_issues_every_queued_request_then_stops_when_the_queue_closes()
    -> Result<(), Box<dyn Error>> {
        let alice = User {
            id: UserId::new(Uuid::new_v4()),
            email: Email::parse("alice@example.test")?,
            display_name: String::new(),
            password_hash: Some(PasswordHash::new("fake:old".into())),
            deactivated_at: None,
        };
        let mailer = FakeMailer::default();
        let service = Arc::new(PasswordResetService::new(
            FakeUsers::with(vec![alice]),
            FakeTokens::default(),
            FakeHasher::default(),
            mailer.clone(),
            FixedClock::at_epoch_plus_days(20_000),
            "http://localhost:5173",
        ));
        let (queue, receiver) = reset_queue(8);

        assert!(queue.enqueue(job("alice@example.test")?));
        assert!(queue.enqueue(job("nobody@example.test")?));
        assert!(queue.enqueue(job("alice@example.test")?));
        drop(queue);
        run_reset_worker(receiver, service).await;

        assert_eq!(mailer.sent().len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn a_full_queue_refuses_instead_of_waiting() -> Result<(), Box<dyn Error>> {
        let (queue, _receiver) = reset_queue(1);

        assert!(queue.enqueue(job("alice@example.test")?));
        assert!(!queue.enqueue(job("bob@example.test")?));
        Ok(())
    }
}
