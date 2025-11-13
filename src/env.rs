use anyhow::Result;
use chashmap::CHashMap;
use rocksdb::{DB, Options};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PersonType {
    Author,
    Committer,
}

/// Unified structure for storing both Authors and Committers in RocksDB
#[derive(Clone, Serialize, Deserialize)]
pub struct PersonRecord {
    pub person_type: PersonType,
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize,
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
}

pub type Authors = CHashMap<String, AuthorInfo>;
pub type Committers = CHashMap<String, CommitterInfo>;
pub type Contributors = HashMap<String, PersonRecord>; // key: "author:hash_hexa" or "committer:hash_hexa"

pub static NOW: OnceLock<i64> = OnceLock::new();
fn get_now_timestamp() -> i64 {
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
    pub unique_file_type_interaction: HashMap<String, usize>, // Counter <.extension, #interactions>
    pub contributors: Contributors,
    pub commit_time: Vec<i64>,
    pub file_system: FileSystem,
}

impl RepositoryStats {
    pub fn new() -> RepositoryStats {
        RepositoryStats {
            unique_file_type_interaction: HashMap::new(),
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
                nom: None,
                email: None,
                number_of_commits: 1,
                number_of_self_commits: { if self_committed { 1 } else { 0 } },
                time_since_first_commit: get_now_timestamp() - date,
                commits: vec![(commit.to_owned(), date, tz)],
            });
    }

    pub fn update_commit_time(&mut self, date: i64) {
        self.commit_time.push(date);
    }

    pub fn flush_to_rocksdb_async_safe(
        &mut self,
        origin: String,
        db: &Arc<Mutex<DB>>,
    ) -> Result<()> {
        self.commit_time.sort_unstable();
        let value = serde_json::to_vec(&self)?;

        // Lock the DB for this write operation
        let db_lock = db.lock().unwrap();
        db_lock.put(origin.as_bytes(), value)?;

        Ok(())
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
