use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, query, query_as, types::Json};

use crate::{ShadowBlockCursor, ShadowBlockRow, ShadowCanonicalRef};

/// Rows written and rows resolved by a single flush.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShadowFlushOutcome {
    /// Rows inserted or updated.
    pub rows_written: usize,
    /// Rows that gained a canonical hash.
    pub rows_reconciled: usize,
}

/// Shadow block repository.
#[derive(Debug)]
pub struct ShadowBlockRepo {
    pool: PgPool,
}

impl ShadowBlockRepo {
    /// Creates a repository.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Persists reorged rows and resolves stored rows from canonical blocks.
    ///
    /// Both run in one transaction, so a reader cannot observe a row written unresolved in the
    /// same flush that resolves it.
    ///
    /// # Errors
    /// Returns an error when the transaction fails.
    pub async fn flush(
        &self,
        rows: &[ShadowBlockRow],
        canonical: &[ShadowCanonicalRef],
    ) -> Result<ShadowFlushOutcome> {
        // Six binds per row; 4,000 stays below Postgres's 65,535-parameter limit.
        const CHUNK_SIZE: usize = 4_000;

        if rows.is_empty() && canonical.is_empty() {
            return Ok(ShadowFlushOutcome::default());
        }

        let deduped = Self::dedupe_last_write_wins(rows);

        let mut tx = self.pool.begin().await.context("failed to begin shadow block transaction")?;
        let mut rows_written = 0usize;

        for chunk in deduped.chunks(CHUNK_SIZE) {
            let mut query_builder: QueryBuilder<'_, Postgres> = QueryBuilder::new(
                "INSERT INTO shadow_blocks \
                 (number, hash, reorged_out, canonical_hash, created_at, payload) ",
            );

            query_builder.push_values(chunk, |mut row, entry| {
                row.push_bind(entry.number)
                    .push_bind(&entry.hash)
                    .push_bind(entry.reorged_out)
                    .push_bind(&entry.canonical_hash)
                    .push_bind(entry.created_at)
                    .push_bind(Json(&entry.payload));
            });

            // `COALESCE` makes `canonical_hash` monotonic. A redelivered notification carries
            // `NULL` and must not erase a hash the backfill already established.
            query_builder.push(
                " ON CONFLICT (number, hash) DO UPDATE SET \
                 reorged_out = EXCLUDED.reorged_out, \
                 canonical_hash = \
                   COALESCE(EXCLUDED.canonical_hash, shadow_blocks.canonical_hash), \
                 payload = EXCLUDED.payload, \
                 updated_at = now()",
            );

            let result = query_builder
                .build()
                .execute(&mut *tx)
                .await
                .context("failed to insert shadow block batch")?;

            rows_written = rows_written.saturating_add(result.rows_affected() as usize);
        }

        let rows_reconciled = Self::resolve_canonical_hashes(&mut tx, canonical).await?;

        tx.commit().await.context("failed to commit shadow block transaction")?;

        Ok(ShadowFlushOutcome { rows_written, rows_reconciled })
    }

    async fn resolve_canonical_hashes(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        canonical: &[ShadowCanonicalRef],
    ) -> Result<usize> {
        if canonical.is_empty() {
            return Ok(0);
        }

        let (numbers, hashes) = Self::dedupe_canonical_last_write_wins(canonical);

        let result = query(
            "UPDATE shadow_blocks AS unresolved \
             SET canonical_hash = canonical.hash, updated_at = now() \
             FROM UNNEST($1::BIGINT[], $2::BYTEA[]) AS canonical(number, hash) \
             WHERE unresolved.number = canonical.number \
               AND unresolved.hash <> canonical.hash \
               AND unresolved.reorged_out \
               AND unresolved.canonical_hash IS NULL",
        )
        .bind(&numbers)
        .bind(&hashes)
        .execute(&mut **tx)
        .await
        .context("failed to resolve canonical hashes for shadow blocks")?;

        Ok(result.rows_affected() as usize)
    }

    /// Postgres picks an arbitrary source row when several `UNNEST` entries match one target, so
    /// a height appearing twice in a flush must collapse to the last hash before binding.
    fn dedupe_canonical_last_write_wins(
        canonical: &[ShadowCanonicalRef],
    ) -> (Vec<i64>, Vec<Vec<u8>>) {
        let mut by_number: HashMap<i64, &[u8]> = HashMap::with_capacity(canonical.len());
        for entry in canonical {
            by_number.insert(entry.number, entry.hash.as_slice());
        }

        by_number.into_iter().map(|(number, hash)| (number, hash.to_vec())).unzip()
    }

    fn dedupe_last_write_wins(rows: &[ShadowBlockRow]) -> Vec<&ShadowBlockRow> {
        let mut by_key: HashMap<(i64, &[u8]), &ShadowBlockRow> = HashMap::with_capacity(rows.len());
        for row in rows {
            by_key.insert((row.number, row.hash.as_slice()), row);
        }
        by_key.into_values().collect()
    }

    /// Lists rows in an inclusive block-number range.
    ///
    /// # Errors
    /// Returns an error when the query fails.
    pub async fn list_by_number_range(&self, start: i64, end: i64) -> Result<Vec<ShadowBlockRow>> {
        let rows = query_as::<_, ShadowBlockRow>(
            "SELECT * FROM shadow_blocks WHERE number BETWEEN $1 AND $2 ORDER BY number, created_at",
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("failed to list shadow blocks by number range")?;

        Ok(rows)
    }

    /// Lists reorged rows after a composite cursor.
    ///
    /// Unresolved rows remain in the query so Rust can advance the cursor past them.
    ///
    /// # Errors
    /// Returns an error on query or payload decode failure.
    pub async fn list_reorged_since(
        &self,
        after: &ShadowBlockCursor,
        limit: i64,
    ) -> Result<Vec<ShadowBlockRow>> {
        let rows = query_as::<_, ShadowBlockRow>(
            "SELECT number, hash, reorged_out, canonical_hash, created_at, updated_at, payload \
             FROM shadow_blocks \
             WHERE reorged_out = true \
               AND (updated_at, number, hash) > ($1, $2, $3) \
             ORDER BY updated_at, number, hash \
             LIMIT $4",
        )
        .bind(after.updated_at)
        .bind(after.number)
        .bind(&after.hash)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("failed to list reorged shadow blocks since cursor")?;

        Ok(rows)
    }

    /// Returns the newest cursor for first-boot initialization.
    ///
    /// # Errors
    /// Returns an error when the query fails.
    pub async fn max_cursor(&self) -> Result<Option<ShadowBlockCursor>> {
        // Include unreconciled rows so first boot cannot replay them later.
        let row = query_as::<_, (DateTime<Utc>, i64, Vec<u8>)>(
            "SELECT updated_at, number, hash FROM shadow_blocks \
             ORDER BY updated_at DESC, number DESC, hash DESC \
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .context("failed to load newest shadow block cursor")?;

        Ok(row.map(|(updated_at, number, hash)| ShadowBlockCursor { updated_at, number, hash }))
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives_traits::RecoveredBlock;

    use super::*;
    use crate::ShadowBlockPayload;

    fn sample_row(number: i64, hash: &[u8], reorged_out: bool) -> ShadowBlockRow {
        ShadowBlockRow {
            number,
            hash: hash.to_vec(),
            reorged_out,
            canonical_hash: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            payload: ShadowBlockPayload {
                builder_version: String::new(),
                block: RecoveredBlock::default(),
                receipts: Vec::new(),
            },
        }
    }

    #[test]
    fn dedupe_collapses_duplicate_number_hash_to_last_write() {
        let rows = vec![
            sample_row(1, &[0xaa], false),
            sample_row(2, &[0xbb], false),
            sample_row(1, &[0xaa], true),
        ];

        let deduped = ShadowBlockRepo::dedupe_last_write_wins(&rows);

        assert_eq!(deduped.len(), 2);
        let kept = deduped
            .iter()
            .find(|row| row.number == 1 && row.hash == [0xaa])
            .expect("duplicated key survives");
        assert!(kept.reorged_out, "duplicate key keeps the last write");
    }

    #[test]
    fn dedupe_keeps_same_number_with_distinct_hash() {
        let rows = vec![sample_row(1, &[0xaa], true), sample_row(1, &[0xbb], false)];

        let deduped = ShadowBlockRepo::dedupe_last_write_wins(&rows);

        assert_eq!(deduped.len(), 2, "distinct hashes at the same height are separate rows");
    }
}
