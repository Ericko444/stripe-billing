use std::future::Future;
use std::sync::Arc;

use argon2::{Algorithm, Argon2, Params, PasswordHasher as _, PasswordVerifier as _, Version};
use identity_domain::{
    NewPassword, Password, PasswordHash, PasswordHashError, PasswordHasher, Verification,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Semaphore;

/// Argon2id memory cost, in KiB (19 MiB).
///
/// The parameters are OWASP's baseline Argon2id configuration
/// (`m = 19456 KiB, t = 2, p = 1`). They are justified here by a memory
/// budget rather than by a benchmark: every hash allocates `m`, and hashing
/// runs behind a [`MAX_CONCURRENT_HASHES`]-permit semaphore, so peak memory
/// spent on hashing is `4 × 19 MiB ≈ 76 MiB` however many logins arrive at
/// once. Excess logins wait for a permit instead of allocating -- which is
/// what keeps the login endpoint from being a memory-exhaustion lever.
/// Raising `m` is a decision to take after measuring on the deployment
/// hardware, and is safe to take later: every stored PHC string records its
/// own parameters, and a successful login rehashes one that is out of date.
pub const ARGON2_MEMORY_KIB: u32 = 19_456;

/// Argon2id iterations. See [`ARGON2_MEMORY_KIB`].
pub const ARGON2_ITERATIONS: u32 = 2;

/// Argon2id lanes. One: each hash runs on a single blocking thread, so
/// extra lanes would add work without adding parallel hardware to do it.
/// Concurrency across requests is the semaphore's job, not the hash's.
pub const ARGON2_PARALLELISM: u32 = 1;

/// Argon2id tag length, in bytes.
pub const ARGON2_OUTPUT_BYTES: usize = 32;

/// How many hashes may run at once. See [`ARGON2_MEMORY_KIB`].
pub const MAX_CONCURRENT_HASHES: usize = 4;

/// Input for the dummy hash. Never a credential: no account can have it as
/// a password, because no account's hash is this one.
const DUMMY_PASSWORD: &[u8] = b"identity-service dummy hash input -- never a credential";

/// Argon2id behind the [`PasswordHasher`] port.
///
/// Hashing is CPU-bound for tens of milliseconds, so it runs in
/// `spawn_blocking` rather than on an async worker thread, where it would
/// stall every other request scheduled there.
#[derive(Clone)]
pub struct Argon2Hasher {
    params: Params,
    permits: Arc<Semaphore>,
    dummy: Arc<PasswordHash>,
}

impl Argon2Hasher {
    /// A hasher with the live parameters. Computes the dummy hash
    /// immediately -- one Argon2id run at construction, which is at startup.
    pub fn new() -> Result<Self, PasswordHashError> {
        Self::with_params(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_PARALLELISM)
    }

    /// A hasher with other parameters -- only for proving that a hash made
    /// under different parameters is detected as needing a rehash.
    #[cfg(test)]
    fn weaker_for_test() -> Result<Self, PasswordHashError> {
        Self::with_params(Params::MIN_M_COST, 1, 1)
    }

    fn with_params(
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
    ) -> Result<Self, PasswordHashError> {
        let params = Params::new(
            memory_kib,
            iterations,
            parallelism,
            Some(ARGON2_OUTPUT_BYTES),
        )
        .map_err(|err| PasswordHashError(err.to_string()))?;
        let dummy = hash_blocking(&params, DUMMY_PASSWORD)?;
        Ok(Self {
            params,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_HASHES)),
            dummy: Arc::new(dummy),
        })
    }

    /// Runs `work` on a blocking thread while holding a hashing permit.
    fn run_bounded<T, F>(
        &self,
        work: F,
    ) -> impl Future<Output = Result<T, PasswordHashError>> + Send + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&Params) -> Result<T, PasswordHashError> + Send + 'static,
    {
        let permits = Arc::clone(&self.permits);
        let params = self.params.clone();
        async move {
            let permit = permits
                .acquire_owned()
                .await
                .map_err(|err| PasswordHashError(err.to_string()))?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                work(&params)
            })
            .await
            .map_err(|err| PasswordHashError(err.to_string()))?
        }
    }
}

impl PasswordHasher for Argon2Hasher {
    fn hash(
        &self,
        password: &NewPassword,
    ) -> impl Future<Output = Result<PasswordHash, PasswordHashError>> + Send {
        // Copied into a fresh secret, zeroed on drop, because the blocking
        // task outlives the borrow.
        let secret = SecretString::from(password.as_password().expose_secret());
        self.run_bounded(move |params| hash_blocking(params, secret.expose_secret().as_bytes()))
    }

    fn verify(
        &self,
        password: &Password,
        stored: &PasswordHash,
    ) -> impl Future<Output = Result<Verification, PasswordHashError>> + Send {
        let secret = SecretString::from(password.expose_secret());
        let stored = stored.as_str().to_string();
        self.run_bounded(move |params| {
            verify_blocking(params, secret.expose_secret().as_bytes(), &stored)
        })
    }

    fn dummy_hash(&self) -> &PasswordHash {
        &self.dummy
    }
}

fn argon2id(params: &Params) -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone())
}

fn hash_blocking(params: &Params, password: &[u8]) -> Result<PasswordHash, PasswordHashError> {
    argon2id(params)
        .hash_password(password)
        .map(|phc| PasswordHash::new(phc.to_string()))
        .map_err(|err| PasswordHashError(err.to_string()))
}

fn verify_blocking(
    params: &Params,
    password: &[u8],
    stored: &str,
) -> Result<Verification, PasswordHashError> {
    let phc =
        argon2::PasswordHash::new(stored).map_err(|err| PasswordHashError(err.to_string()))?;

    // `verify_password` hashes with the parameters recorded in `phc`, not
    // with `params` -- which is what lets an old hash still verify.
    match argon2id(params).verify_password(password, &phc) {
        Ok(()) if uses_live_parameters(params, &phc) => Ok(Verification::Match),
        Ok(()) => Ok(Verification::MatchNeedsRehash),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(Verification::Mismatch),
        Err(err) => Err(PasswordHashError(err.to_string())),
    }
}

fn uses_live_parameters(live: &Params, phc: &argon2::PasswordHash) -> bool {
    let Ok(recorded) = Params::try_from(phc) else {
        return false;
    };
    phc.algorithm.as_str() == argon2::ARGON2ID_IDENT.as_str()
        && phc.version == Some(Version::V0x13.into())
        && recorded.m_cost() == live.m_cost()
        && recorded.t_cost() == live.t_cost()
        && recorded.p_cost() == live.p_cost()
        && recorded.output_len() == live.output_len()
}

#[cfg(test)]
mod tests {
    use identity_domain::{NewPassword, Password};

    use super::*;

    const LIVE_PARAMS: &str = "$argon2id$v=19$m=19456,t=2,p=1$";

    fn new_password(raw: &str) -> Result<NewPassword, Box<dyn std::error::Error>> {
        Ok(NewPassword::check(Password::new(raw.to_string()))?)
    }

    #[tokio::test]
    async fn the_stored_hash_records_the_configured_parameters()
    -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let hash = hasher
            .hash(&new_password("correct horse battery staple")?)
            .await?;

        assert!(hash.as_str().starts_with(LIVE_PARAMS), "{}", hash.as_str());
        Ok(())
    }

    #[tokio::test]
    async fn the_right_password_matches_and_a_wrong_one_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let hash = hasher
            .hash(&new_password("correct horse battery staple")?)
            .await?;

        let right = Password::new("correct horse battery staple".to_string());
        let wrong = Password::new("correct horse battery stapler".to_string());
        assert_eq!(hasher.verify(&right, &hash).await?, Verification::Match);
        assert_eq!(hasher.verify(&wrong, &hash).await?, Verification::Mismatch);
        Ok(())
    }

    #[tokio::test]
    async fn two_hashes_of_one_password_differ_by_salt() -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let first = hasher
            .hash(&new_password("correct horse battery staple")?)
            .await?;
        let second = hasher
            .hash(&new_password("correct horse battery staple")?)
            .await?;

        assert_ne!(first.as_str(), second.as_str());
        Ok(())
    }

    /// The unknown-address path is only as expensive as the known-address
    /// path if the dummy hash costs what a real one does -- which it does
    /// exactly when it was made with the live parameters.
    #[tokio::test]
    async fn the_dummy_hash_uses_the_live_parameters_and_matches_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let dummy = hasher.dummy_hash().clone();

        assert!(
            dummy.as_str().starts_with(LIVE_PARAMS),
            "{}",
            dummy.as_str()
        );
        let guess = Password::new("correct horse battery staple".to_string());
        assert_eq!(hasher.verify(&guess, &dummy).await?, Verification::Mismatch);
        Ok(())
    }

    #[tokio::test]
    async fn a_hash_made_with_other_parameters_needs_rehash()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = Argon2Hasher::weaker_for_test()?;
        let hash = old
            .hash(&new_password("correct horse battery staple")?)
            .await?;

        let live = Argon2Hasher::new()?;
        let password = Password::new("correct horse battery staple".to_string());
        assert_eq!(
            live.verify(&password, &hash).await?,
            Verification::MatchNeedsRehash
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_malformed_stored_hash_is_an_error_not_a_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let password = Password::new("correct horse battery staple".to_string());
        let result = hasher
            .verify(
                &password,
                &PasswordHash::new("not a phc string".to_string()),
            )
            .await;

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn debug_does_not_print_the_hash() -> Result<(), Box<dyn std::error::Error>> {
        let hasher = Argon2Hasher::new()?;
        let rendered = format!("{:?}", hasher.dummy_hash());
        assert!(!rendered.contains("argon2id"));
        Ok(())
    }
}
