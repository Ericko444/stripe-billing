//! Integration tests for `migrations`, against a disposable Postgres.

mod common;

use std::error::Error;

#[tokio::test]
async fn applying_migrations_twice_is_a_no_op() -> Result<(), Box<dyn Error>> {
    let db = common::setup().await?;

    persistence::run_migrations(&db.pool).await?;

    Ok(())
}
