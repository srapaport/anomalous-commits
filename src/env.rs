use chashmap::CHashMap;

#[derive(Clone)]
pub struct AuthorInfo {
    pub nom: Option<String>,
    pub email: Option<String>,
    pub number_of_commits: usize,
    pub time_since_first_commit: i64,
    pub commits: Vec<(String, i64, i16)>,
}

pub type Authors = CHashMap<String, AuthorInfo>;

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
