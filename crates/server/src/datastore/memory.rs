use super::{DataStore, DataStoreError, InitialLeaf};
use futures::Stream;
use indexmap::IndexMap;
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::Arc,
};
use tokio::sync::RwLock;
use warg_crypto::hash::AnyHash;
use warg_protocol::{
    operator, package,
    registry::{LogId, LogLeaf, MapCheckpoint, RecordId},
    ProtoEnvelope, SerdeEnvelope,
};

struct Log<V, R> {
    validator: V,
    entries: Vec<ProtoEnvelope<R>>,
    checkpoint_indices: Vec<usize>,
}

impl<V, R> Default for Log<V, R>
where
    V: Default,
{
    fn default() -> Self {
        Self {
            validator: V::default(),
            entries: Vec::new(),
            checkpoint_indices: Vec::new(),
        }
    }
}

struct Record {
    /// Index in the log's entries.
    index: usize,
    /// Index in the checkpoints map.
    checkpoint_index: Option<usize>,
}

enum PendingRecord {
    Operator {
        record: Option<ProtoEnvelope<operator::OperatorRecord>>,
    },
    Package {
        record: Option<ProtoEnvelope<package::PackageRecord>>,
        missing: HashSet<AnyHash>,
    },
}

enum RejectedRecord {
    Operator {
        record: ProtoEnvelope<operator::OperatorRecord>,
        reason: String,
    },
    Package {
        record: ProtoEnvelope<package::PackageRecord>,
        reason: String,
    },
}

enum RecordStatus {
    Pending(PendingRecord),
    Rejected(RejectedRecord),
    Validated(Record),
}

#[derive(Default)]
struct State {
    operators: HashMap<LogId, Log<operator::Validator, operator::OperatorRecord>>,
    packages: HashMap<LogId, Log<package::Validator, package::PackageRecord>>,
    checkpoints: IndexMap<AnyHash, SerdeEnvelope<MapCheckpoint>>,
    records: HashMap<LogId, HashMap<RecordId, RecordStatus>>,
}

fn get_records_before_checkpoint(indices: &[usize], checkpoint_index: usize) -> usize {
    indices
        .iter()
        .filter(|index| **index <= checkpoint_index)
        .count()
}

/// Represents an in-memory data store.
///
/// Data is not persisted between restarts of the server.
///
/// Note: this is mainly used for testing, so it is not very efficient as
/// it shares a single RwLock for all operations.
pub struct MemoryDataStore(Arc<RwLock<State>>);

impl MemoryDataStore {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(State::default())))
    }
}

impl Default for MemoryDataStore {
    fn default() -> Self {
        Self::new()
    }
}

#[axum::async_trait]
impl DataStore for MemoryDataStore {
    async fn get_names(&self) -> Result<Vec<Option<String>>, DataStoreError> {
      let foo = Vec::new();
      Ok(foo)
    }
    
    async fn get_initial_leaves(
        &self,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<InitialLeaf, DataStoreError>> + Send>>,
        DataStoreError,
    > {
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn store_operator_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
        record: &ProtoEnvelope<operator::OperatorRecord>,
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;
        let prev = state.records.entry(log_id.clone()).or_default().insert(
            record_id.clone(),
            RecordStatus::Pending(PendingRecord::Operator {
                record: Some(record.clone()),
            }),
        );

        assert!(prev.is_none());
        Ok(())
    }

    async fn reject_operator_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
        reason: &str,
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;

        let status = state
            .records
            .get_mut(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get_mut(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        let record = match status {
            RecordStatus::Pending(PendingRecord::Operator { record }) => record.take().unwrap(),
            _ => return Err(DataStoreError::RecordNotPending(record_id.clone())),
        };

        *status = RecordStatus::Rejected(RejectedRecord::Operator {
            record,
            reason: reason.to_string(),
        });

        Ok(())
    }

    async fn validate_operator_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;

        let State {
            operators, records, ..
        } = &mut *state;

        let status = records
            .get_mut(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get_mut(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        match status {
            RecordStatus::Pending(PendingRecord::Operator { record }) => {
                let record = record.take().unwrap();
                let log = operators.entry(log_id.clone()).or_default();
                match log
                    .validator
                    .validate(&record)
                    .map_err(DataStoreError::from)
                {
                    Ok(_) => {
                        let index = log.entries.len();
                        log.entries.push(record);
                        *status = RecordStatus::Validated(Record {
                            index,
                            checkpoint_index: None,
                        });
                        Ok(())
                    }
                    Err(e) => {
                        *status = RecordStatus::Rejected(RejectedRecord::Operator {
                            record,
                            reason: e.to_string(),
                        });
                        Err(e)
                    }
                }
            }
            _ => Err(DataStoreError::RecordNotPending(record_id.clone())),
        }
    }

    async fn store_package_record(
        &self,
        log_id: &LogId,
        _name: &str,
        record_id: &RecordId,
        record: &ProtoEnvelope<package::PackageRecord>,
        missing: &HashSet<&AnyHash>,
    ) -> Result<(), DataStoreError> {
        // Ensure the set of missing hashes is a subset of the record contents.
        debug_assert!({
            use warg_protocol::Record;
            let contents = record.as_ref().contents();
            missing.is_subset(&contents)
        });

        let mut state = self.0.write().await;
        let prev = state.records.entry(log_id.clone()).or_default().insert(
            record_id.clone(),
            RecordStatus::Pending(PendingRecord::Package {
                record: Some(record.clone()),
                missing: missing.iter().map(|&d| d.clone()).collect(),
            }),
        );

        assert!(prev.is_none());
        Ok(())
    }

    async fn reject_package_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
        reason: &str,
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;

        let status = state
            .records
            .get_mut(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get_mut(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        let record = match status {
            RecordStatus::Pending(PendingRecord::Package { record, .. }) => record.take().unwrap(),
            _ => return Err(DataStoreError::RecordNotPending(record_id.clone())),
        };

        *status = RecordStatus::Rejected(RejectedRecord::Package {
            record,
            reason: reason.to_string(),
        });

        Ok(())
    }

    async fn validate_package_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;

        let State {
            packages, records, ..
        } = &mut *state;

        let status = records
            .get_mut(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get_mut(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        match status {
            RecordStatus::Pending(PendingRecord::Package { record, .. }) => {
                let record = record.take().unwrap();
                let log = packages.entry(log_id.clone()).or_default();
                match log
                    .validator
                    .validate(&record)
                    .map_err(DataStoreError::from)
                {
                    Ok(_) => {
                        let index = log.entries.len();
                        log.entries.push(record);
                        *status = RecordStatus::Validated(Record {
                            index,
                            checkpoint_index: None,
                        });
                        Ok(())
                    }
                    Err(e) => {
                        *status = RecordStatus::Rejected(RejectedRecord::Package {
                            record,
                            reason: e.to_string(),
                        });
                        Err(e)
                    }
                }
            }
            _ => Err(DataStoreError::RecordNotPending(record_id.clone())),
        }
    }

    async fn is_content_missing(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
        digest: &AnyHash,
    ) -> Result<bool, DataStoreError> {
        let state = self.0.read().await;
        let log = state
            .records
            .get(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

        let status = log
            .get(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        match status {
            RecordStatus::Pending(PendingRecord::Operator { .. }) => {
                // Operator records have no content
                Ok(false)
            }
            RecordStatus::Pending(PendingRecord::Package { missing, .. }) => {
                Ok(missing.contains(digest))
            }
            _ => return Err(DataStoreError::RecordNotPending(record_id.clone())),
        }
    }

    async fn set_content_present(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
        digest: &AnyHash,
    ) -> Result<bool, DataStoreError> {
        let mut state = self.0.write().await;
        let log = state
            .records
            .get_mut(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

        let status = log
            .get_mut(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        match status {
            RecordStatus::Pending(PendingRecord::Operator { .. }) => {
                // Operator records have no content, so conceptually already present
                Ok(false)
            }
            RecordStatus::Pending(PendingRecord::Package { missing, .. }) => {
                if missing.is_empty() {
                    return Ok(false);
                }

                // Return true if this was the last missing content
                missing.remove(digest);
                Ok(missing.is_empty())
            }
            _ => return Err(DataStoreError::RecordNotPending(record_id.clone())),
        }
    }

    async fn store_checkpoint(
        &self,
        checkpoint_id: &AnyHash,
        checkpoint: SerdeEnvelope<MapCheckpoint>,
        participants: &[LogLeaf],
    ) -> Result<(), DataStoreError> {
        let mut state = self.0.write().await;

        let (index, prev) = state
            .checkpoints
            .insert_full(checkpoint_id.clone(), checkpoint);
        assert!(prev.is_none());

        for leaf in participants {
            if let Some(log) = state.operators.get_mut(&leaf.log_id) {
                log.checkpoint_indices.push(index);
            } else if let Some(log) = state.packages.get_mut(&leaf.log_id) {
                log.checkpoint_indices.push(index);
            } else {
                unreachable!("log not found");
            }

            match state
                .records
                .get_mut(&leaf.log_id)
                .unwrap()
                .get_mut(&leaf.record_id)
                .unwrap()
            {
                RecordStatus::Validated(record) => {
                    record.checkpoint_index = Some(index);
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    async fn get_latest_checkpoint(&self) -> Result<SerdeEnvelope<MapCheckpoint>, DataStoreError> {
        let state = self.0.read().await;
        let checkpoint = state.checkpoints.values().last().unwrap();
        Ok(checkpoint.clone())
    }

    async fn get_operator_records(
        &self,
        log_id: &LogId,
        root: &AnyHash,
        since: Option<&RecordId>,
        limit: u16,
    ) -> Result<Vec<ProtoEnvelope<operator::OperatorRecord>>, DataStoreError> {
        let state = self.0.read().await;

        let log = state
            .operators
            .get(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

        if let Some(checkpoint_index) = state.checkpoints.get_index_of(root) {
            let start = match since {
                Some(since) => match &state.records[log_id][since] {
                    RecordStatus::Validated(record) => record.index + 1,
                    _ => unreachable!(),
                },
                None => 0,
            };

            let end = get_records_before_checkpoint(&log.checkpoint_indices, checkpoint_index);
            Ok(log.entries[start..std::cmp::min(end, start + limit as usize)].to_vec())
        } else {
            Err(DataStoreError::CheckpointNotFound(root.clone()))
        }
    }

    async fn get_package_records(
        &self,
        log_id: &LogId,
        root: &AnyHash,
        since: Option<&RecordId>,
        limit: u16,
    ) -> Result<Vec<ProtoEnvelope<package::PackageRecord>>, DataStoreError> {
        let state = self.0.read().await;

        let log = state
            .packages
            .get(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

        if let Some(checkpoint_index) = state.checkpoints.get_index_of(root) {
            let start = match since {
                Some(since) => match &state.records[log_id][since] {
                    RecordStatus::Validated(record) => record.index + 1,
                    _ => unreachable!(),
                },
                None => 0,
            };

            let end = get_records_before_checkpoint(&log.checkpoint_indices, checkpoint_index);
            Ok(log.entries[start..std::cmp::min(end, start + limit as usize)].to_vec())
        } else {
            Err(DataStoreError::CheckpointNotFound(root.clone()))
        }
    }

    async fn get_operator_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
    ) -> Result<super::Record<operator::OperatorRecord>, DataStoreError> {
        let state = self.0.read().await;
        let status = state
            .records
            .get(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        let (status, envelope, checkpoint) = match status {
            RecordStatus::Pending(PendingRecord::Operator { record, .. }) => {
                (super::RecordStatus::Pending, record.clone().unwrap(), None)
            }
            RecordStatus::Rejected(RejectedRecord::Operator { record, reason }) => (
                super::RecordStatus::Rejected(reason.into()),
                record.clone(),
                None,
            ),
            RecordStatus::Validated(r) => {
                let log = state
                    .operators
                    .get(log_id)
                    .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

                let checkpoint = r.checkpoint_index.map(|i| state.checkpoints[i].clone());

                (
                    if checkpoint.is_some() {
                        super::RecordStatus::Published
                    } else {
                        super::RecordStatus::Validated
                    },
                    log.entries[r.index].clone(),
                    checkpoint,
                )
            }
            _ => return Err(DataStoreError::RecordNotFound(record_id.clone())),
        };

        Ok(super::Record {
            status,
            envelope,
            checkpoint,
        })
    }

    async fn get_package_record(
        &self,
        log_id: &LogId,
        record_id: &RecordId,
    ) -> Result<super::Record<package::PackageRecord>, DataStoreError> {
        let state = self.0.read().await;
        let status = state
            .records
            .get(log_id)
            .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?
            .get(record_id)
            .ok_or_else(|| DataStoreError::RecordNotFound(record_id.clone()))?;

        let (status, envelope, checkpoint) = match status {
            RecordStatus::Pending(PendingRecord::Package { record, .. }) => {
                (super::RecordStatus::Pending, record.clone().unwrap(), None)
            }
            RecordStatus::Rejected(RejectedRecord::Package { record, reason }) => (
                super::RecordStatus::Rejected(reason.into()),
                record.clone(),
                None,
            ),
            RecordStatus::Validated(r) => {
                let log = state
                    .packages
                    .get(log_id)
                    .ok_or_else(|| DataStoreError::LogNotFound(log_id.clone()))?;

                let checkpoint = r.checkpoint_index.map(|i| state.checkpoints[i].clone());

                (
                    if checkpoint.is_some() {
                        super::RecordStatus::Published
                    } else {
                        super::RecordStatus::Validated
                    },
                    log.entries[r.index].clone(),
                    checkpoint,
                )
            }
            _ => return Err(DataStoreError::RecordNotFound(record_id.clone())),
        };

        Ok(super::Record {
            status,
            envelope,
            checkpoint,
        })
    }
}
