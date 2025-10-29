use std::collections::HashMap;

use chashmap::CHashMap;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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
pub struct RepositoryStats {
    files_added: usize,
    files_removed: usize,
    files_modified: usize,
    files_renamed: usize,
    unique_file_type_interaction: HashMap<String, usize>, // Counter <.extension, #interactions>

    contributors: Contributors,
    commit_time: Vec<i64>,
    time_between_commit: Vec<usize>,
}

impl RepositoryStats {
    pub fn new() -> RepositoryStats {
        RepositoryStats {
            files_added: 0,
            files_removed: 0,
            files_modified: 0,
            files_renamed: 0,
            unique_file_type_interaction: HashMap::new(),
            contributors: Contributors::new(),
            commit_time: Vec::new(),
            time_between_commit: Vec::new(),
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
                person_type: PersonType::Author,
                nom: None,
                email: None,
                number_of_commits: 1,
                number_of_self_commits: { if self_committed { 1 } else { 0 } },
                time_since_first_commit: get_now_timestamp() - date,
                commits: vec![(commit.to_owned(), date, tz)],
            });
    }
}
