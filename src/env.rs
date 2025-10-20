use chashmap::CHashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthorInfo {
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize,// (author == committer)
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CommitterInfo {
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub number_of_self_commits: usize,// (author == committer)
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

// 13 columns
// id ok
// msg ok
// author ok
// date ok
// tz ok
// ??
// committer ok
// cdate ok
// ctz ok
// ??
// directory ok
// visit type ok
// ??
