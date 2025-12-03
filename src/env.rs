use anyhow::Result;
use chashmap::CHashMap;
use rocksdb::{DB, Options};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, OnceLock};

pub const SENSITIVE_FILE_EXTENSIONS: &[&str] = &[
    ".xml",
    ".json",
    ".jar",
    ".ini",
    ".dat",
    ".cnf",
    ".yml",
    ".toml",
    ".gradle",
    ".bin",
    ".config",
    ".exe",
    ".properties",
    ".cmd",
    ".build",
];

pub const BINARY_EXTENSIONS: &[&str] = &[
    ".exe", ".dll", // Executables and libraries
    ".o",   // Compiled object files
    ".jar", // Java archives
    ".bin", ".dat", // Generic binary
];

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthorInfo {
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize, // (author == committer)
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CommitterInfo {
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize, // (author == committer)
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
}

/// Enum to differentiate between Author and Committer
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PersonType {
    Author,
    Committer,
}

/// Unified structure for storing both Authors and Committers in RocksDB
#[derive(Clone, Serialize, Deserialize)]
pub struct PersonRecord {
    pub person_type: PersonType,
    pub name: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize,
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
    pub extensions: HashMap<String, usize>, // <extension, #interactions>
}

pub type Authors = CHashMap<String, AuthorInfo>;
pub type Committers = CHashMap<String, CommitterInfo>;
pub type Contributors = HashMap<String, PersonRecord>; // key: "author:hash_hexa" or "committer:hash_hexa"

pub static NOW: OnceLock<i64> = OnceLock::new();
pub fn get_now_timestamp() -> i64 {
    *NOW.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub creator: String,
    pub interactions: HashMap<String, usize>, // <author, amount of interactions>
    pub ownership_score: HashMap<String, f64>, // <author, score>
    pub commit_time: Vec<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FileSystem {
    pub files: HashMap<String, FileInfo>,
    pub files_added: usize,
    pub files_modified: usize,
    pub files_removed: usize,
    pub files_renamed: usize,
}

impl FileSystem {
    pub fn new() -> FileSystem {
        FileSystem {
            files: HashMap::new(),
            files_added: 0,
            files_modified: 0,
            files_removed: 0,
            files_renamed: 0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RepositoryStats {
    pub file_type_interaction: HashMap<String, usize>, // Counter <.extension, #interactions>
    pub contributors: Contributors,
    pub commit_time: Vec<i64>,
    pub file_system: FileSystem,
}

impl RepositoryStats {
    pub fn new() -> RepositoryStats {
        RepositoryStats {
            file_type_interaction: HashMap::new(),
            contributors: Contributors::new(),
            commit_time: Vec::new(),
            file_system: FileSystem::new(),
        }
    }

    pub fn update_contributors(
        &mut self,
        contributor_id: String,
        commit: &String,
        date: i64,
        tz: i16,
        self_committed: bool,
    ) {
        let person_type = {
            if contributor_id.contains("committer") {
                PersonType::Committer
            } else {
                PersonType::Author
            }
        };
        self.contributors
            .entry(contributor_id)
            .and_modify(|pr| {
                pr.number_of_commits += 1;
                if self_committed {
                    pr.number_of_self_commits += 1;
                }
                pr.time_since_first_commit =
                    pr.time_since_first_commit.max(get_now_timestamp() - date);
                pr.commits.push((commit.to_owned(), date, tz));
            })
            .or_insert_with(|| PersonRecord {
                person_type: person_type,
                name: None,
                email: None,
                number_of_commits: 1,
                number_of_self_commits: { if self_committed { 1 } else { 0 } },
                time_since_first_commit: get_now_timestamp() - date,
                commits: vec![(commit.to_owned(), date, tz)],
                extensions: HashMap::new(),
            });
    }

    pub fn update_commit_time(&mut self, date: i64) {
        self.commit_time.push(date);
    }

    pub fn flush_to_rocksdb_async_safe(
        &mut self,
        origin: String,
        db: &Arc<Mutex<DB>>,
        max_size: &AtomicUsize,
    ) -> Result<()> {
        self.commit_time.sort_unstable();
        let value = serde_json::to_vec(&self)?;

        max_size.fetch_max(value.len(), std::sync::atomic::Ordering::Relaxed);
        // Lock the DB for this write operation
        let db_lock = db.lock().unwrap();
        db_lock.put(origin.as_bytes(), value)?;

        Ok(())
    }

    /// Flush to RocksDB with defensive programming for large values
    /// Returns (was_skipped, was_truncated)
    pub fn flush_to_rocksdb_async_safe_defensive(
        &mut self,
        origin: String,
        db: &Arc<Mutex<DB>>,
        max_record_size: usize,
        max_size: &(Arc<Mutex<String>>, AtomicUsize),
    ) -> Result<(bool, bool)> {
        self.commit_time.sort_unstable();
        let mut value = serde_json::to_vec(&self)?;
        let mut was_truncated = false;
        let length = value.len();
        if length > max_size.1.load(std::sync::atomic::Ordering::Relaxed) {
            max_size
                .1
                .fetch_max(length, std::sync::atomic::Ordering::Relaxed);
            let mut max_url = max_size.0.lock().unwrap();
            *max_url = origin.clone();
        }
        // Check if the value is too large
        if length > max_record_size {
            log::warn!(
                "Repo {} has {} commits, serialized size: {:.2} MB - truncating commit_time",
                origin,
                self.commit_time.len(),
                length as f64 / (1024.0 * 1024.0)
            );

            // Truncate commit_time to first and last 1000 entries
            if self.commit_time.len() > 2000 {
                let mut truncated_times = Vec::with_capacity(2000);
                truncated_times.extend_from_slice(&self.commit_time[..1000]);
                truncated_times
                    .extend_from_slice(&self.commit_time[self.commit_time.len() - 1000..]);
                self.commit_time = truncated_times;
            } else {
                // Not enough data to truncate meaningfully, skip this repo
                log::error!(
                    "Repo {} too large but has only {} commits - skipping",
                    origin,
                    self.commit_time.len()
                );
                return Ok((true, false)); // skipped, not truncated
            }

            value = serde_json::to_vec(&self)?;

            // If STILL too large after truncation, skip this repo
            if value.len() > max_record_size {
                log::error!(
                    "Repo {} still too large after truncation ({:.2} MB) - skipping",
                    origin,
                    value.len() as f64 / (1024.0 * 1024.0)
                );
                return Ok((true, false)); // skipped, not truncated
            }
            was_truncated = true;
        }

        // Lock the DB for this write operation
        let db_lock = db.lock().unwrap();
        db_lock.put(origin.as_bytes(), value)?;

        Ok((false, was_truncated)) // not skipped, maybe truncated
    }

    pub fn flush_to_rocksdb(&mut self, origin: String, db_path: &str) -> Result<()> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, db_path)?;
        self.commit_time.sort_unstable();
        let value = serde_json::to_vec(&self)?;
        db.put(origin.as_bytes(), value)?;
        db.flush()?;
        Ok(())
    }
}
