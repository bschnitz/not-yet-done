//! The timesheet index: the user's own timesheets as last seen, on disk.
//!
//! Kimai's listing leaves out records on projects the user's teams no longer
//! reach — closed projects, typically — although `GET /api/timesheets/{id}`
//! still serves them. Every listing therefore records what it got here, so a
//! record that later drops out of the listing is still known by id and can be
//! fetched on its own (see [`super::hidden`]).
//!
//! One JSON file per instance; every change runs under an exclusive lock on a
//! sibling `.lock` file and is written to a temporary file first, so the TUI
//! and a CLI scan running side by side neither interleave nor tear the file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::client::KimaiTimesheet;

/// The index contents.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub(super) struct TimesheetIndex {
    /// The user's own records by id, as last seen.
    #[serde(default)]
    pub entries: BTreeMap<u64, KimaiTimesheet>,
    /// Ids a scan found to be someone else's or gone, so the next scan
    /// skips them.
    #[serde(default)]
    pub foreign: BTreeSet<u64>,
}

/// Where the index lives, and the locked read/update around it.
#[derive(Clone, Debug)]
pub(super) struct IndexStore {
    path: PathBuf,
}

impl IndexStore {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The current contents; an index that does not exist yet is empty.
    pub(super) async fn read(&self) -> Result<TimesheetIndex, String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock(&path, false)?;
            load(&path)
        })
        .await
        .map_err(|e| format!("timesheet index: {e}"))?
    }

    /// Apply `change` to the current contents and write the result back.
    pub(super) async fn update<F>(&self, change: F) -> Result<(), String>
    where
        F: FnOnce(&mut TimesheetIndex) + Send + 'static,
    {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock(&path, true)?;
            let mut index = load(&path)?;
            change(&mut index);
            save(&path, &index)
        })
        .await
        .map_err(|e| format!("timesheet index: {e}"))?
    }
}

fn io_err(path: &Path, e: io::Error) -> String {
    format!("timesheet index {}: {e}", path.display())
}

/// Lock the sibling `.lock` file; the lock lasts as long as the handle.
fn lock(path: &Path, exclusive: bool) -> Result<File, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    }
    let lock_path = path.with_extension("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| io_err(&lock_path, e))?;
    if exclusive {
        file.lock()
    } else {
        file.lock_shared()
    }
    .map_err(|e| io_err(&lock_path, e))?;
    Ok(file)
}

fn load(path: &Path) -> Result<TimesheetIndex, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "timesheet index {} is unreadable ({e}); move it away to start over",
                path.display()
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(TimesheetIndex::default()),
        Err(e) => Err(io_err(path, e)),
    }
}

fn save(path: &Path, index: &TimesheetIndex) -> Result<(), String> {
    let text = serde_json::to_string(index).map_err(|e| format!("timesheet index: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| io_err(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| io_err(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> KimaiTimesheet {
        KimaiTimesheet {
            id,
            project: 7,
            activity: 3,
            begin: "2030-01-15T09:00:00+0100".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_missing_index_reads_as_empty_and_updates_persist() {
        let dir = tempfile::tempdir().unwrap();
        let store = IndexStore::new(dir.path().join("sub").join("index.json"));
        assert!(store.read().await.unwrap().entries.is_empty());

        store
            .update(|idx| {
                idx.entries.insert(1, entry(1));
                idx.foreign.insert(2);
            })
            .await
            .unwrap();
        store
            .update(|idx| {
                idx.foreign.insert(3);
            })
            .await
            .unwrap();

        let idx = store.read().await.unwrap();
        assert_eq!(idx.entries.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(idx.foreign.iter().copied().collect::<Vec<_>>(), vec![2, 3]);
    }

    #[tokio::test]
    async fn a_corrupt_index_is_an_error_not_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");
        fs::write(&path, "{ not json").unwrap();
        let err = IndexStore::new(path).read().await.unwrap_err();
        assert!(err.contains("unreadable"), "{err}");
    }
}
