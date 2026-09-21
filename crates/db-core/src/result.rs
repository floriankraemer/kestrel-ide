//! `ResultSet`: the accumulated pages of one execution's rows, capped at
//! `memory_cap_mib` (database-tools.md §8) — the NFR table's "fetch stops
//! within one batch of the cap" requirement (ADR-0058), verified here by
//! `append_batch`'s own unit test rather than left to the nightly 1M-row
//! bench alone.

use crate::value::RowBatch;

/// A memory budget in bytes. Constructed from MiB since that is how
/// `database-tools.md` §8's `memory_cap_mib` setting is spelled — the
/// conversion happens once, here, rather than at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryCap {
    bytes: usize,
}

impl MemoryCap {
    pub fn from_mib(mib: u32) -> Self {
        Self {
            bytes: (mib as usize) * 1024 * 1024,
        }
    }

    pub fn bytes(self) -> usize {
        self.bytes
    }
}

/// [`ResultSet::append_batch`] refused a further batch: the cap was
/// already reached by what came before, and appending this batch would
/// only grow it further. Not a [`crate::error::DbError`] — running out of
/// local buffer space is not the backend's problem, and "Fetch more" after
/// clearing older pages is a normal, expected next step, not a failure to
/// report as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapReached;

/// The batches fetched so far for one execution, and how much of the
/// memory cap they have used.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    batches: Vec<RowBatch>,
    used_bytes: usize,
}

impl ResultSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `batch` if the cap allows it. The cap is checked *before*
    /// accounting for `batch` — once `used_bytes` has already reached the
    /// cap, fetching stops rather than accepting one more batch and only
    /// then reporting it over; this is what "stops within one batch of the
    /// cap" means: the batch that would be the first one *over* is the one
    /// refused, not accepted and left to overshoot further next time.
    pub fn append_batch(&mut self, batch: RowBatch, cap: MemoryCap) -> Result<(), CapReached> {
        // The very first batch is always accepted, even against a
        // zero-byte cap — fetching has to return *something* for the
        // first page, and "the cap was already full before anything was
        // fetched" is not a state this type can be in.
        if !self.batches.is_empty() && self.used_bytes >= cap.bytes() {
            return Err(CapReached);
        }
        self.used_bytes += batch.approx_size();
        self.batches.push(batch);
        Ok(())
    }

    pub fn row_count(&self) -> usize {
        self.batches.iter().map(|batch| batch.rows.len()).sum()
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn batches(&self) -> &[RowBatch] {
        &self.batches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{ColumnMeta, Value};

    fn batch(row_count: usize) -> RowBatch {
        RowBatch {
            columns: vec![ColumnMeta {
                name: "n".to_string(),
                type_name: "int".to_string(),
                nullable: false,
            }],
            rows: (0..row_count).map(|i| vec![Value::Int(i as i64)]).collect(),
        }
    }

    #[test]
    fn memory_cap_converts_mib_to_bytes() {
        assert_eq!(MemoryCap::from_mib(1).bytes(), 1024 * 1024);
    }

    #[test]
    fn appending_under_the_cap_succeeds_and_tracks_row_count() {
        let mut result = ResultSet::new();
        let cap = MemoryCap::from_mib(1);
        result.append_batch(batch(10), cap).expect("under cap");
        assert_eq!(result.row_count(), 10);
        assert!(result.used_bytes() > 0);
    }

    #[test]
    fn appending_once_the_cap_is_reached_is_refused() {
        let mut result = ResultSet::new();
        // A cap of zero bytes: the very first batch already meets it.
        let cap = MemoryCap { bytes: 0 };
        assert_eq!(result.append_batch(batch(1), cap), Ok(()));
        assert_eq!(result.append_batch(batch(1), cap), Err(CapReached));
        // The first batch was still accepted (fetching stopped within one
        // batch of the cap, not before any data at all).
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn a_generous_cap_never_refuses() {
        let mut result = ResultSet::new();
        let cap = MemoryCap::from_mib(256);
        for _ in 0..50 {
            result
                .append_batch(batch(100), cap)
                .expect("well under cap");
        }
        assert_eq!(result.row_count(), 5000);
    }
}
