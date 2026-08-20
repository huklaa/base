//! Postgres-backed system tests for shadow block retention sweeps.

use std::time::Duration;

use anyhow::Result;
use base_shadow_indexer_db::{
    SHADOW_RETENTION_LOCK_KEY, ShadowBlockCursor, ShadowBlockPayload, ShadowBlockRepo,
    ShadowBlockRow, ShadowDbConfig, ShadowMetricsCursorRepo, ShadowRetentionRepo,
};
use chrono::{DateTime, Utc};
use reth_primitives_traits::RecoveredBlock;
use sqlx::{PgPool, query, query_scalar};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;

const RETENTION_PERIOD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Matches the RDS engine version the shadow builders write to. The module default is Postgres
/// 11, which predates the `MATERIALIZED` CTE hint the sweep relies on.
const POSTGRES_TAG: &str = "17-alpine";

struct TestDatabase {
    _container: ContainerAsync<Postgres>,
    pool: PgPool,
}

impl TestDatabase {
    async fn start() -> Result<Self> {
        let container = Postgres::default().with_tag(POSTGRES_TAG).start().await?;
        let port = container.get_host_port_ipv4(5432).await?;
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool =
            ShadowDbConfig { url, max_connections: 5, connection_timeout: Duration::from_secs(5) }
                .init_pool()
                .await?;

        Ok(Self { _container: container, pool })
    }

    fn retention(&self) -> ShadowRetentionRepo {
        ShadowRetentionRepo::new(self.pool.clone())
    }

    fn blocks(&self) -> ShadowBlockRepo {
        ShadowBlockRepo::new(self.pool.clone())
    }

    async fn age_all_rows(&self, age: Duration) -> Result<()> {
        let seconds = i64::try_from(age.as_secs())?;
        query("UPDATE shadow_blocks SET updated_at = now() - ($1::bigint * interval '1 second')")
            .bind(seconds)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn age_row(&self, number: i64, age: Duration) -> Result<()> {
        let seconds = i64::try_from(age.as_secs())?;
        query(
            "UPDATE shadow_blocks SET updated_at = now() - ($1::bigint * interval '1 second') \
             WHERE number = $2",
        )
        .bind(seconds)
        .bind(number)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn remaining_numbers(&self) -> Result<Vec<i64>> {
        let numbers = query_scalar("SELECT number FROM shadow_blocks ORDER BY number")
            .fetch_all(&self.pool)
            .await?;

        Ok(numbers)
    }
}

fn shadow_row(number: i64, reorged_out: bool) -> ShadowBlockRow {
    let now = Utc::now();

    ShadowBlockRow {
        number,
        hash: number.to_be_bytes().to_vec(),
        reorged_out,
        canonical_hash: None,
        created_at: now,
        updated_at: now,
        payload: ShadowBlockPayload {
            builder_version: "retention-test".to_string(),
            block: RecoveredBlock::default(),
            receipts: Vec::new(),
        },
    }
}

const fn days(count: u64) -> Duration {
    Duration::from_secs(count * 24 * 60 * 60)
}

#[tokio::test]
async fn deletes_only_rows_older_than_the_retention_period() -> Result<()> {
    let database = TestDatabase::start().await?;
    let blocks = database.blocks();
    blocks
        .insert_batch(&[shadow_row(1, false), shadow_row(2, false), shadow_row(3, false)])
        .await?;

    database.age_row(1, days(8)).await?;
    database.age_row(2, days(30)).await?;

    let retention = database.retention();
    let cutoff = retention.cutoff(RETENTION_PERIOD).await?;
    let sweep = retention.sweep(cutoff).await?.expect("sweep should take the retention lock");

    assert_eq!(sweep.deleted, 2);
    assert!(!sweep.capped);
    assert_eq!(database.remaining_numbers().await?, vec![3]);

    Ok(())
}

#[tokio::test]
async fn keeps_every_row_inside_the_retention_period() -> Result<()> {
    let database = TestDatabase::start().await?;
    let blocks = database.blocks();
    blocks.insert_batch(&[shadow_row(1, false), shadow_row(2, false)]).await?;

    database.age_all_rows(days(6)).await?;

    let retention = database.retention();
    let cutoff = retention.cutoff(RETENTION_PERIOD).await?;
    let sweep = retention.sweep(cutoff).await?.expect("sweep should take the retention lock");

    assert_eq!(sweep.deleted, 0);
    assert_eq!(sweep.batches, 0);
    assert_eq!(database.remaining_numbers().await?, vec![1, 2]);

    Ok(())
}

#[tokio::test]
async fn deletes_a_large_backlog_across_several_batches() -> Result<()> {
    const EXPIRED_ROWS: i64 = 2_500;

    let database = TestDatabase::start().await?;
    let rows: Vec<ShadowBlockRow> =
        (1..=EXPIRED_ROWS).map(|number| shadow_row(number, false)).collect();
    database.blocks().insert_batch(&rows).await?;
    database.age_all_rows(days(8)).await?;

    let retention = database.retention();
    let cutoff = retention.cutoff(RETENTION_PERIOD).await?;
    let sweep = retention.sweep(cutoff).await?.expect("sweep should take the retention lock");

    assert_eq!(sweep.deleted, u64::try_from(EXPIRED_ROWS)?);
    assert!(sweep.batches >= 2, "a backlog past one batch must span several transactions");
    assert!(database.remaining_numbers().await?.is_empty());

    Ok(())
}

#[tokio::test]
async fn yields_when_another_builder_holds_the_retention_lock() -> Result<()> {
    let database = TestDatabase::start().await?;
    database.blocks().insert_batch(&[shadow_row(1, false)]).await?;
    database.age_all_rows(days(8)).await?;

    let mut holder = database.pool.acquire().await?;
    holder.close_on_drop();
    let held: bool = query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(SHADOW_RETENTION_LOCK_KEY)
        .fetch_one(&mut *holder)
        .await?;
    assert!(held, "the test must hold the lock before the sweep runs");

    let retention = database.retention();
    let cutoff = retention.cutoff(RETENTION_PERIOD).await?;

    assert!(retention.sweep(cutoff).await?.is_none());
    assert_eq!(database.remaining_numbers().await?, vec![1]);

    Ok(())
}

#[tokio::test]
async fn reports_expired_rows_the_metrics_cursor_never_consumed() -> Result<()> {
    let database = TestDatabase::start().await?;
    database.blocks().insert_batch(&[shadow_row(1, true)]).await?;
    database.age_all_rows(days(8)).await?;

    let retention = database.retention();
    let cutoff = retention.cutoff(RETENTION_PERIOD).await?;

    assert!(
        !retention.has_unread_expired(cutoff).await?,
        "an absent cursor means the reader never ran, which is not an unread-row signal"
    );

    let cursor = ShadowMetricsCursorRepo::new(database.pool.clone());
    cursor.store(&ShadowBlockCursor::genesis()).await?;

    assert!(
        retention.has_unread_expired(cutoff).await?,
        "a cursor behind the expiring reorged row must be reported"
    );

    let updated_at =
        query_scalar::<_, DateTime<Utc>>("SELECT updated_at FROM shadow_blocks WHERE number = 1")
            .fetch_one(&database.pool)
            .await?;
    cursor
        .store(&ShadowBlockCursor { updated_at, number: 1, hash: 1i64.to_be_bytes().to_vec() })
        .await?;

    assert!(
        !retention.has_unread_expired(cutoff).await?,
        "a cursor level with the expiring row leaves nothing unread"
    );

    Ok(())
}
