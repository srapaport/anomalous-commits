use anyhow::Result;
use rocksdb::{DB, IteratorMode, Options};
use std::collections::{HashMap, HashSet};
use swh_graph::NodeType;
use swh_graph::graph::SwhFullGraph;

use crate::diff::{DiffStats, compute_diff_stats, get_root_dir};
use crate::env::{
    self, BINARY_EXTENSIONS, PersonRecord, RepositoryStats, SENSITIVE_FILE_EXTENSIONS,
};
use crate::time_of_day::{Commit, TimeOfDayProfile};

pub struct CommitInfo {
    pub timestamp: i64,
    pub tz_offset: i16,
}

pub struct AnomalyScore {
    pub large_addition: f64,
    pub large_deletion: f64,
    pub large_modification: f64,
    pub large_moves: f64,
    pub time_of_day: f64,
    pub author_tod: f64,
    pub committer_tod: f64,
    pub file_type_interaction: f64,
    pub file_type_interaction_author: f64,
    pub number_commits_author: f64,
    pub number_commits_committer: f64,
    pub newcomer_author: f64,
    pub newcomer_committer: f64,
    pub account_age_author: f64,
    pub account_age_committer: f64,
    pub repository_dormancy: f64,
    pub sensitive_file: f64,
    pub prebuilt_file: f64,
    pub ownership_score: (HashMap<String, f64>, bool),
    pub total_score: f64,
}

/// Weights for each anomaly factor used in computing the final weighted score
#[derive(Debug, Clone)]
pub struct AnomalyWeights {
    pub large_addition: i32,
    pub large_deletion: i32,
    pub large_modification: i32,
    pub large_moves: i32,
    pub time_of_day: i32,
    pub author_tod: i32,
    pub committer_tod: i32,
    pub file_type_interaction: i32,
    pub file_type_interaction_author: i32,
    pub number_commits_author: i32,
    pub number_commits_committer: i32,
    pub newcomer_author: i32,
    pub newcomer_committer: i32,
    pub account_age_author: i32,
    pub account_age_committer: i32,
    pub repository_dormancy: i32,
    pub sensitive_file: i32,
    pub prebuilt_file: i32,
    pub ownership_score: i32,
}

impl Default for AnomalyWeights {
    fn default() -> Self {
        Self {
            large_addition: 1,
            large_deletion: 1,
            large_modification: 1,
            large_moves: 1,
            time_of_day: 1,
            author_tod: 1,
            committer_tod: 1,
            file_type_interaction: 1,
            file_type_interaction_author: 1,
            number_commits_author: 1,
            number_commits_committer: 1,
            newcomer_author: 1,
            newcomer_committer: 1,
            account_age_author: 1,
            account_age_committer: 1,
            repository_dormancy: 1,
            sensitive_file: 1,
            prebuilt_file: 1,
            ownership_score: 1,
        }
    }
}

impl AnomalyWeights {
    /// Calculate the sum of all weights
    pub fn sum(&self) -> i32 {
        self.large_addition
            + self.large_deletion
            + self.large_modification
            + self.large_moves
            + self.time_of_day
            + self.author_tod
            + self.committer_tod
            + self.file_type_interaction
            + self.file_type_interaction_author
            + self.number_commits_author
            + self.number_commits_committer
            + self.newcomer_author
            + self.newcomer_committer
            + self.account_age_author
            + self.account_age_committer
            + self.repository_dormancy
            + self.sensitive_file
            + self.prebuilt_file
            + self.ownership_score
    }
}

/// Compute the final weighted anomaly score
///
/// Takes an AnomalyScore and weights, multiplies each factor by its weight,
/// sums them up, and normalizes by the sum of weights to keep scores comparable.
pub fn compute_weighted_score(score: &AnomalyScore, weights: &AnomalyWeights) -> f64 {
    let mut total = 0.0;

    total += score.large_addition * weights.large_addition as f64;
    total += score.large_deletion * weights.large_deletion as f64;
    total += score.large_modification * weights.large_modification as f64;
    total += score.large_moves * weights.large_moves as f64;
    total += score.time_of_day * weights.time_of_day as f64;
    total += score.author_tod * weights.author_tod as f64;
    total += score.committer_tod * weights.committer_tod as f64;
    total += score.file_type_interaction * weights.file_type_interaction as f64;
    total += score.file_type_interaction_author * weights.file_type_interaction_author as f64;
    total += score.number_commits_author * weights.number_commits_author as f64;
    total += score.number_commits_committer * weights.number_commits_committer as f64;
    total += score.newcomer_author * weights.newcomer_author as f64;
    total += score.newcomer_committer * weights.newcomer_committer as f64;
    total += score.account_age_author * weights.account_age_author as f64;
    total += score.account_age_committer * weights.account_age_committer as f64;
    total += score.repository_dormancy * weights.repository_dormancy as f64;
    total += score.sensitive_file * weights.sensitive_file as f64;
    total += score.prebuilt_file * weights.prebuilt_file as f64;

    // For ownership_score, use the max anomaly score (the bool indicates if it's anomalous)
    if score.ownership_score.1 {
        if let Some(max_score) = score
            .ownership_score
            .0
            .values()
            .max_by(|a, b| a.partial_cmp(b).unwrap())
        {
            total += max_score * weights.ownership_score as f64;
        }
    }

    // Normalize by the sum of weights
    let weight_sum = weights.sum() as f64;
    if weight_sum > 0.0 {
        total / weight_sum
    } else {
        0.0
    }
}

/// Calculate time-of-day anomaly score based on repository commit patterns
fn compute_time_of_day_score(commit_info: &CommitInfo, repo_stats: &RepositoryStats) -> f64 {
    // Build time-of-day profile from all commits in the repository
    let commits: Vec<Commit> = repo_stats
        .contributors
        .values()
        .flat_map(|person| {
            person.commits.iter().map(|(id, ts, tz)| Commit {
                id: id.clone(),
                timestamp: *ts * 1_000_000_000, // Convert to nanoseconds
                tz_offset: *tz,
            })
        })
        .collect();

    if commits.is_empty() {
        return 0.0;
    }

    let mut profile = TimeOfDayProfile::new();
    profile.learn(&commits);

    // Create commit object for the current commit
    let current_commit = Commit {
        id: String::new(),                                // ID not needed for scoring
        timestamp: commit_info.timestamp * 1_000_000_000, // Convert to nanoseconds
        tz_offset: commit_info.tz_offset,
    };
    profile.get_anomaly_score(&current_commit)
}

/// Calculate time-of-day anomaly score based on repository commit patterns
fn compute_time_of_day_score_person(commit_info: &CommitInfo, person: &PersonRecord) -> f64 {
    // Build time-of-day profile from all commits in the repository
    let commits: Vec<Commit> = person
        .commits
        .iter()
        .map(|commit| Commit {
            id: commit.0.clone(),
            timestamp: commit.1 * 1_000_000_000,
            tz_offset: commit.2,
        })
        .collect();

    if commits.is_empty() {
        return 0.0;
    }

    let mut profile = TimeOfDayProfile::new();
    profile.learn(&commits);

    // Create commit object for the current commit
    let current_commit = Commit {
        id: String::new(),                                // ID not needed for scoring
        timestamp: commit_info.timestamp * 1_000_000_000, // Convert to nanoseconds
        tz_offset: commit_info.tz_offset,
    };

    profile.get_anomaly_score(&current_commit)
}

#[derive(Debug, Clone, Default)]
pub struct RepoStatistics {
    mean_added: f64,
    mean_deleted: f64,
    mean_modified: f64,
    mean_renamed: f64,
    q1_q2_commits_per_author: (f64, f64),
    q1_q2_commits_per_committer: (f64, f64),
    q1_q2_time_since_first_commit_author: (f64, f64),
    q1_q2_time_since_first_commit_committer: (f64, f64),
}

#[derive(Debug, Clone, Default)]
pub struct GlobalStatistics {
    pub q1_q2_account_age_author: (f64, f64),
    pub q1_q2_account_age_committer: (f64, f64),
}

/// Compute global account age statistics from a RocksDB database containing PersonRecords
/// Returns Q1 and Q2 (25th and 50th percentiles) for both authors and committers
pub fn compute_global_account_age_statistics(db_path: &str) -> Result<GlobalStatistics> {
    let mut opts = Options::default();
    opts.create_if_missing(false);
    let db = DB::open_for_read_only(&opts, db_path, false)?;

    let mut author_ages: Vec<i64> = Vec::new();
    let mut committer_ages: Vec<i64> = Vec::new();

    let now = env::get_now_timestamp();

    // Iterate through all entries in the database
    let iter = db.iterator(IteratorMode::Start);

    for item in iter {
        let (key, value) = item?;
        let key_str = String::from_utf8_lossy(&key);

        // Deserialize PersonRecord
        let person_record: PersonRecord = match serde_json::from_slice(&value) {
            Ok(record) => record,
            Err(e) => {
                log::warn!(
                    "Failed to deserialize PersonRecord for key {}: {}",
                    key_str,
                    e
                );
                continue;
            }
        };

        // Find the earliest commit timestamp (first commit globally)
        if let Some(&earliest_timestamp) = person_record.commits.iter().map(|(_, ts, _)| ts).min() {
            let account_age = now - earliest_timestamp;
            match person_record.person_type {
                env::PersonType::Author => author_ages.push(account_age),
                env::PersonType::Committer => committer_ages.push(account_age),
            }
        }
    }

    author_ages.sort_unstable();
    committer_ages.sort_unstable();

    // Compute Q1 and Q2 for authors
    let q1_q2_account_age_author = if author_ages.is_empty() {
        (0.0, 0.0)
    } else {
        let n = author_ages.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (author_ages[q1_index - 1] + author_ages[q1_index]) as f64 / 2.0
        } else {
            author_ages[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (author_ages[n / 2 - 1] + author_ages[n / 2]) as f64 / 2.0
        } else {
            author_ages[n / 2] as f64
        };

        (q1, q2)
    };

    // Compute Q1 and Q2 for committers
    let q1_q2_account_age_committer = if committer_ages.is_empty() {
        (0.0, 0.0)
    } else {
        let n = committer_ages.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (committer_ages[q1_index - 1] + committer_ages[q1_index]) as f64 / 2.0
        } else {
            committer_ages[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (committer_ages[n / 2 - 1] + committer_ages[n / 2]) as f64 / 2.0
        } else {
            committer_ages[n / 2] as f64
        };

        (q1, q2)
    };

    log::info!(
        "Global statistics computed: {} authors, {} committers",
        author_ages.len(),
        committer_ages.len()
    );
    log::info!(
        "Author account age Q1={:.2} days, Q2={:.2} days",
        q1_q2_account_age_author.0 / 86400.0,
        q1_q2_account_age_author.1 / 86400.0
    );
    log::info!(
        "Committer account age Q1={:.2} days, Q2={:.2} days",
        q1_q2_account_age_committer.0 / 86400.0,
        q1_q2_account_age_committer.1 / 86400.0
    );

    Ok(GlobalStatistics {
        q1_q2_account_age_author,
        q1_q2_account_age_committer,
    })
}

/// Compute statistics from repository history (to be done only once)
pub fn compute_repo_statistics(repo_stats: &RepositoryStats) -> RepoStatistics {
    let total_commits = repo_stats.commit_time.len() as f64;

    if total_commits == 0.0 {
        return RepoStatistics::default();
    }

    let mean_added = repo_stats.file_system.files_added as f64 / total_commits;
    let mean_deleted = repo_stats.file_system.files_removed as f64 / total_commits;
    let mean_modified = repo_stats.file_system.files_modified as f64 / total_commits;
    let mean_renamed = repo_stats.file_system.files_renamed as f64 / total_commits;

    let mut commits_per_author: Vec<usize> = Vec::new();
    let mut commits_per_committer: Vec<usize> = Vec::new();
    let mut time_since_first_author: Vec<i64> = Vec::new();
    let mut time_since_first_committer: Vec<i64> = Vec::new();

    for (_, pr) in &repo_stats.contributors {
        if pr.person_type == env::PersonType::Author {
            commits_per_author.push(pr.number_of_commits);
            time_since_first_author.push(pr.time_since_first_commit);
        } else {
            commits_per_committer.push(pr.number_of_commits);
            time_since_first_committer.push(pr.time_since_first_commit);
        }
    }

    commits_per_author.sort_unstable();
    commits_per_committer.sort_unstable();

    // Compute Q1 (25th percentile) and Q2 (median) for authors
    let q1_q2_commits_per_author = if commits_per_author.is_empty() {
        (0.0, 0.0)
    } else {
        let n = commits_per_author.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (commits_per_author[q1_index - 1] + commits_per_author[q1_index]) as f64 / 2.0
        } else {
            commits_per_author[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (commits_per_author[n / 2 - 1] + commits_per_author[n / 2]) as f64 / 2.0
        } else {
            commits_per_author[n / 2] as f64
        };

        (q1, q2)
    };

    // Compute Q1 (25th percentile) and Q2 (median) for committers
    let q1_q2_commits_per_committer = if commits_per_committer.is_empty() {
        (0.0, 0.0)
    } else {
        let n = commits_per_committer.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (commits_per_committer[q1_index - 1] + commits_per_committer[q1_index]) as f64 / 2.0
        } else {
            commits_per_committer[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (commits_per_committer[n / 2 - 1] + commits_per_committer[n / 2]) as f64 / 2.0
        } else {
            commits_per_committer[n / 2] as f64
        };

        (q1, q2)
    };

    // Sort time_since_first_commit data
    time_since_first_author.sort_unstable();
    time_since_first_committer.sort_unstable();

    // Compute Q1 and Q2 for time_since_first_commit (authors)
    let q1_q2_time_since_first_commit_author = if time_since_first_author.is_empty() {
        (0.0, 0.0)
    } else {
        let n = time_since_first_author.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (time_since_first_author[q1_index - 1] + time_since_first_author[q1_index]) as f64 / 2.0
        } else {
            time_since_first_author[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (time_since_first_author[n / 2 - 1] + time_since_first_author[n / 2]) as f64 / 2.0
        } else {
            time_since_first_author[n / 2] as f64
        };

        (q1, q2)
    };

    // Compute Q1 and Q2 for time_since_first_commit (committers)
    let q1_q2_time_since_first_commit_committer = if time_since_first_committer.is_empty() {
        (0.0, 0.0)
    } else {
        let n = time_since_first_committer.len();

        // Q1 at 25th percentile
        let q1_index = n / 4;
        let q1 = if n % 4 == 0 && q1_index > 0 {
            (time_since_first_committer[q1_index - 1] + time_since_first_committer[q1_index]) as f64
                / 2.0
        } else {
            time_since_first_committer[q1_index] as f64
        };

        // Q2 at 50th percentile (median)
        let q2 = if n % 2 == 0 {
            (time_since_first_committer[n / 2 - 1] + time_since_first_committer[n / 2]) as f64 / 2.0
        } else {
            time_since_first_committer[n / 2] as f64
        };

        (q1, q2)
    };

    RepoStatistics {
        mean_added,
        mean_deleted,
        mean_modified,
        mean_renamed,
        q1_q2_commits_per_author,
        q1_q2_commits_per_committer,
        q1_q2_time_since_first_commit_author,
        q1_q2_time_since_first_commit_committer,
    }
}

/// Aux function for compute_diff_anomaly_scores
fn compute_distance_score(value: usize, mean: f64) -> f64 {
    if mean == 0.0 {
        return if value == 0 { 0.0 } else { 1.0 };
    }

    let distance = value as f64 - mean;
    if distance < 0.0 {
        // No large kind of anything
        return 0.0;
    }
    // Cap at 1.0 for very large deviations
    (distance / mean).min(1.0)
}

/// Compute individual anomaly scores for a commit's diff statistics
fn compute_diff_anomaly_scores(
    diff_stats: &DiffStats,
    repo_means: &RepoStatistics,
) -> (f64, f64, f64, f64) {
    let large_addition = compute_distance_score(diff_stats.added, repo_means.mean_added);
    let large_deletion = compute_distance_score(diff_stats.deleted, repo_means.mean_deleted);
    let large_modification = compute_distance_score(diff_stats.modified, repo_means.mean_modified);
    let large_moves = compute_distance_score(diff_stats.renamed, repo_means.mean_renamed);

    (
        large_addition,
        large_deletion,
        large_modification,
        large_moves,
    )
}

/// Compute file type interaction anomaly score
/// Analyzes the distribution of file extensions touched in a commit
/// compared to the repository's historical patterns
fn compute_file_type_anomaly_score(
    commit_extensions: &HashMap<String, usize>,
    repo_stats: &RepositoryStats,
) -> f64 {
    let total_commits = repo_stats.commit_time.len() as f64;

    if total_commits == 0.0 || commit_extensions.is_empty() {
        return 0.0;
    }

    // Compute total interactions in this commit
    let commit_total_interactions: usize = commit_extensions.values().sum();

    if commit_total_interactions == 0 {
        return 0.0;
    }

    // Compute mean interactions per extension per commit
    let mut extension_means: HashMap<String, f64> = HashMap::new();
    for (ext, total_interactions) in &repo_stats.file_type_interaction {
        extension_means.insert(ext.clone(), *total_interactions as f64 / total_commits);
    }

    // Compute anomaly score based on deviation from mean
    let mut anomaly_scores: Vec<f64> = Vec::new();

    for (ext, &commit_count) in commit_extensions {
        let mean = extension_means.get(ext).copied().unwrap_or(0.0);

        // Compute distance score for this extension
        let score = if mean == 0.0 {
            // New extension not seen before in repo
            1.0
        } else {
            let distance = commit_count as f64 - mean;
            if distance < 0.0 {
                0.0
            } else {
                (distance / mean).min(1.0)
            }
        };

        anomaly_scores.push(score);
    }

    // Return average anomaly across all extensions in this commit
    if anomaly_scores.is_empty() {
        0.0
    } else {
        anomaly_scores.iter().sum::<f64>() / anomaly_scores.len() as f64
    }
}

fn compute_file_type_anomaly_score_author(
    commit_extensions: &HashMap<String, usize>,
    author: &PersonRecord,
) -> f64 {
    let total_commits = author.number_of_commits as f64;

    if total_commits == 0.0 || commit_extensions.is_empty() {
        return 0.0;
    }

    // Compute mean interactions per extension per commit
    let mut extension_means: HashMap<String, f64> = HashMap::new();
    for (ext, total_interactions) in &author.extensions {
        extension_means.insert(ext.clone(), *total_interactions as f64 / total_commits);
    }

    // Compute total interactions in this commit
    let commit_total_interactions: usize = commit_extensions.values().sum();

    if commit_total_interactions == 0 {
        return 0.0;
    }

    // Compute anomaly score based on deviation from mean
    let mut anomaly_scores: Vec<f64> = Vec::new();

    for (ext, &commit_count) in commit_extensions {
        let mean = extension_means.get(ext).copied().unwrap_or(0.0);

        // Compute distance score for this extension
        let score = if mean == 0.0 {
            // New extension not seen before in repo
            1.0
        } else {
            let distance = commit_count as f64 - mean;
            if distance < 0.0 {
                0.0
            } else {
                (distance / mean).min(1.0)
            }
        };

        anomaly_scores.push(score);
    }

    // Return average anomaly across all extensions in this commit
    if anomaly_scores.is_empty() {
        0.0
    } else {
        anomaly_scores.iter().sum::<f64>() / anomaly_scores.len() as f64
    }
}

fn compute_anomaly_ownership(
    repo_stats: &RepositoryStats,
    author_id: &String,
    final_added: &HashMap<String, usize>,
    deleted_files: &HashMap<String, HashSet<usize>>,
    modified_files: &HashMap<String, usize>,
    renamed_files: &HashMap<String, (String, usize)>,
) -> (HashMap<String, f64>, bool) {
    let mut minority_files = HashMap::new();
    let mut one_anomaly = false;
    fn compute_map<V>(
        repo_stats: &RepositoryStats,
        author_id: &String,
        minority_files: &mut HashMap<String, f64>,
        one_anomaly: &mut bool,
        list_file: &HashMap<String, V>,
    ) {
        for path in list_file.keys() {
            let Some(file_info) = repo_stats.file_system.files.get(path) else {
                continue;
            };
            if file_info.creator == *author_id {
                continue;
            }
            let Some(biggest_contrib) =
                file_info
                    .ownership_score
                    .values()
                    .fold(None, |max, &x| match max {
                        None => Some(x),
                        Some(y) => Some(x.max(y)),
                    })
            else {
                continue;
            };
            let threshold = biggest_contrib / 2.0;
            if let Some(score) = file_info.ownership_score.get(author_id)
                && *score < threshold
            {
                let anomaly_score = (threshold - score) / threshold;
                minority_files.insert(path.to_string(), anomaly_score);
                *one_anomaly = true;
            }
        }
    }
    compute_map(
        repo_stats,
        author_id,
        &mut minority_files,
        &mut one_anomaly,
        final_added,
    );
    compute_map(
        repo_stats,
        author_id,
        &mut minority_files,
        &mut one_anomaly,
        deleted_files,
    );
    compute_map(
        repo_stats,
        author_id,
        &mut minority_files,
        &mut one_anomaly,
        modified_files,
    );
    compute_map(
        repo_stats,
        author_id,
        &mut minority_files,
        &mut one_anomaly,
        renamed_files,
    );
    return (minority_files, one_anomaly);
}

fn compute_number_commits_anomaly_author(
    repo_means: &RepoStatistics,
    author: &PersonRecord,
) -> f64 {
    let num_commits = author.number_of_commits as f64;
    let q1 = repo_means.q1_q2_commits_per_author.0;
    let q2 = repo_means.q1_q2_commits_per_author.1;

    if num_commits >= q2 {
        // Established contributor (above median)
        return 0.0;
    }

    if num_commits <= q1 {
        // Very few commits (below Q1)
        return 1.0;
    }

    // Linear interpolation between Q1 and Q2
    // Score decreases as number of commits increases
    let range = q2 - q1;
    if range == 0.0 {
        return 0.0;
    }

    (q2 - num_commits) / range
}

fn compute_number_commits_anomaly_committer(
    repo_means: &RepoStatistics,
    committer: &PersonRecord,
) -> f64 {
    let num_commits = committer.number_of_commits as f64;
    let q1 = repo_means.q1_q2_commits_per_committer.0;
    let q2 = repo_means.q1_q2_commits_per_committer.1;

    if num_commits >= q2 {
        // Established contributor (above median)
        return 0.0;
    }

    if num_commits <= q1 {
        // Very few commits (below Q1)
        return 1.0;
    }

    // Score decreases as number of commits increases
    let range = q2 - q1;
    if range == 0.0 {
        return 0.0;
    }

    (q2 - num_commits) / range
}

/// Compute newcomer anomaly score based on time_since_first_commit
/// Returns higher score for newer contributors (those with less time since first commit)
fn compute_newcomer_anomaly(person: &PersonRecord, q1: f64, q2: f64) -> f64 {
    let time_since_first = person.time_since_first_commit as f64;

    if time_since_first >= q2 {
        // Established contributor (above median)
        return 0.0;
    }

    if time_since_first <= q1 {
        // Very new contributor (below Q1)
        return 1.0;
    }

    // Score decreases as time_since_first increases
    let range = q2 - q1;
    if range == 0.0 {
        return 0.0;
    }

    (q2 - time_since_first) / range
}

/// Compute global account age anomaly score based on earliest commit across all repositories
/// Returns higher score for accounts with recent creation dates (low account age)
fn compute_account_age_anomaly(person: &PersonRecord, global_q1: f64, global_q2: f64) -> f64 {
    let earliest_commit = person.commits.iter().map(|(_, ts, _)| ts).min();

    let Some(&first_commit_ts) = earliest_commit else {
        return 1.0; // No commits = suspicious/unknown
    };

    let now = env::get_now_timestamp();
    let account_age = (now - first_commit_ts) as f64;

    if account_age >= global_q2 {
        // Established account (above median age)
        return 0.0;
    }

    if account_age <= global_q1 {
        // Very new account (below Q1)
        return 1.0;
    }

    // Linear interpolation between Q1 and Q2
    // Score decreases as account age increases
    let range = global_q2 - global_q1;
    if range == 0.0 {
        return 0.0;
    }

    (global_q2 - account_age) / range
}

/// Compute repository dormancy score based on activity trend
/// Compares commits in last 6 months vs 6-12 months ago
/// Returns 0.0 for active/growing repositories, 1.0 for dormant repositories
fn compute_repository_dormancy_score(repo_stats: &RepositoryStats) -> f64 {
    if repo_stats.commit_time.is_empty() {
        return 1.0; // No commits = completely dormant
    }

    let now = env::get_now_timestamp();
    const SIX_MONTHS: i64 = 180 * 24 * 60 * 60;
    const ONE_YEAR: i64 = 365 * 24 * 60 * 60;

    // Count commits in last 6 months
    let recent_commits = repo_stats
        .commit_time
        .iter()
        .filter(|&&ts| now - ts <= SIX_MONTHS)
        .count() as f64;

    // Count commits between 6-12 months ago
    let previous_commits = repo_stats
        .commit_time
        .iter()
        .filter(|&&ts| {
            let age = now - ts;
            age > SIX_MONTHS && age <= ONE_YEAR
        })
        .count() as f64;

    // Handle edge cases
    if previous_commits == 0.0 && recent_commits == 0.0 {
        return 1.0; // No activity in last year = dormant
    }

    if previous_commits == 0.0 && recent_commits > 0.0 {
        return 0.0; // New activity = active
    }

    // Calculate ratio of recent to previous activity
    // If recent >= previous: activity maintained or increased → score = 0.0
    // If recent < previous: activity decreased → score based on decline
    let ratio = recent_commits / previous_commits;

    if ratio >= 1.0 {
        // Activity maintained or increased
        return 0.0;
    }

    // Activity decreased: score = 1.0 - ratio
    // ratio = 0.5 → score = 0.5 (50% decline)
    // ratio = 0.0 → score = 1.0 (complete decline)
    1.0 - ratio
}

pub fn compute_score<G: SwhFullGraph>(
    commit: usize,
    repo_stats: &RepositoryStats,
    repo_means: &RepoStatistics,
    global_stats: Option<&GlobalStatistics>,
    graph: &G,
) -> Option<AnomalyScore> {
    // Create CommitInfo
    let Some(author) = graph.properties().author_id(commit) else {
        return None;
    };
    let author_id = format!("author:{:064x}", author);
    let Some(committer) = graph.properties().committer_id(commit) else {
        return None;
    };
    let committer_id = format!("committer:{:064x}", committer);
    let Some(cdate) = graph.properties().committer_timestamp(commit) else {
        return None;
    };
    let Some(ctz) = graph.properties().committer_timestamp_offset(commit) else {
        return None;
    };
    let commit_info = CommitInfo {
        timestamp: cdate,
        tz_offset: ctz,
    };

    // Get commit root directory
    let commit_root_dir = get_root_dir(commit, graph)?;

    // Get parent commits
    let parents: Vec<usize> = graph
        .successors(commit)
        .into_iter()
        .filter(|&succ| graph.properties().node_type(succ) == NodeType::Revision)
        .collect();

    // Get parent root directories
    let parent_root_dirs: Vec<usize> = parents
        .iter()
        .filter_map(|&parent| get_root_dir(parent, graph))
        .collect();

    // Compute diff stats
    let (diff_stats, final_added, deleted_files, modified_files, renamed_files) =
        compute_diff_stats(commit_root_dir, &parent_root_dirs, graph);

    // Compute anomaly scores based on diff stats
    let (large_addition, large_deletion, large_modification, large_moves) =
        compute_diff_anomaly_scores(&diff_stats, repo_means);

    // Collect file extensions touched in this commit
    let mut commit_extensions: HashMap<String, usize> = HashMap::new();

    let mut has_sensitive = false;
    let mut has_prebuilt = false;

    // From added files
    for (path, _) in &final_added {
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            *commit_extensions.entry(ext.to_string()).or_insert(0) += 1;
            if !has_sensitive && SENSITIVE_FILE_EXTENSIONS.contains(&ext) {
                has_sensitive = true;
            }
            if !has_prebuilt && BINARY_EXTENSIONS.contains(&ext) {
                has_prebuilt = true;
            }
        }
    }

    // From deleted files
    for (path, _) in &deleted_files {
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            *commit_extensions.entry(ext.to_string()).or_insert(0) += 1;
            if !has_sensitive && SENSITIVE_FILE_EXTENSIONS.contains(&ext) {
                has_sensitive = true;
            }
            // Not checking for deletion of prebuilts
        }
    }

    // From modified files
    for (path, _) in &modified_files {
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            *commit_extensions.entry(ext.to_string()).or_insert(0) += 1;
            if !has_sensitive && SENSITIVE_FILE_EXTENSIONS.contains(&ext) {
                has_sensitive = true;
            }
            if !has_prebuilt && BINARY_EXTENSIONS.contains(&ext) {
                has_prebuilt = true;
            }
        }
    }

    // From renamed files (use new path)
    for (new_path, _) in &renamed_files {
        if let Some(ext) = std::path::Path::new(new_path)
            .extension()
            .and_then(|e| e.to_str())
        {
            *commit_extensions.entry(ext.to_string()).or_insert(0) += 1;
            if !has_sensitive && SENSITIVE_FILE_EXTENSIONS.contains(&ext) {
                has_sensitive = true;
            }
            if !has_prebuilt && BINARY_EXTENSIONS.contains(&ext) {
                has_prebuilt = true;
            }
        }
    }

    let file_type_anomaly_repo = compute_file_type_anomaly_score(&commit_extensions, repo_stats);

    let author_anomalies = if let Some(author) = repo_stats.contributors.get(&author_id) {
        (
            compute_file_type_anomaly_score_author(&commit_extensions, author),
            compute_number_commits_anomaly_author(repo_means, author),
            compute_newcomer_anomaly(
                author,
                repo_means.q1_q2_time_since_first_commit_author.0,
                repo_means.q1_q2_time_since_first_commit_author.1,
            ),
        )
    } else {
        (0.0, 0.0, 0.0)
    };

    let committer_anomalies = if let Some(committer) = repo_stats.contributors.get(&committer_id) {
        (
            compute_number_commits_anomaly_committer(repo_means, committer),
            compute_newcomer_anomaly(
                committer,
                repo_means.q1_q2_time_since_first_commit_committer.0,
                repo_means.q1_q2_time_since_first_commit_committer.1,
            ),
        )
    } else {
        (0.0, 0.0)
    };

    // Compute global account age anomalies if global statistics are provided
    let account_age_author = if let Some(global_stats) = global_stats {
        if let Some(author) = repo_stats.contributors.get(&author_id) {
            compute_account_age_anomaly(
                author,
                global_stats.q1_q2_account_age_author.0,
                global_stats.q1_q2_account_age_author.1,
            )
        } else {
            0.0
        }
    } else {
        0.0
    };

    let account_age_committer = if let Some(global_stats) = global_stats {
        if let Some(committer) = repo_stats.contributors.get(&committer_id) {
            compute_account_age_anomaly(
                committer,
                global_stats.q1_q2_account_age_committer.0,
                global_stats.q1_q2_account_age_committer.1,
            )
        } else {
            0.0
        }
    } else {
        0.0
    };

    let ownership_score = compute_anomaly_ownership(
        repo_stats,
        &author_id,
        &final_added,
        &deleted_files,
        &modified_files,
        &renamed_files,
    );

    let author_tod = if let Some(author) = repo_stats.contributors.get(&author_id) {
        compute_time_of_day_score_person(&commit_info, author)
    } else {
        0.0
    };
    let committer_tod = if let Some(committer) = repo_stats.contributors.get(&committer_id) {
        compute_time_of_day_score_person(&commit_info, committer)
    } else {
        0.0
    };

    let repository_dormancy = compute_repository_dormancy_score(repo_stats);

    Some(AnomalyScore {
        large_addition,
        large_deletion,
        large_modification,
        large_moves,
        time_of_day: compute_time_of_day_score(&commit_info, repo_stats),
        author_tod,
        committer_tod,
        file_type_interaction: file_type_anomaly_repo,
        file_type_interaction_author: author_anomalies.0,
        number_commits_author: author_anomalies.1,
        number_commits_committer: committer_anomalies.0,
        newcomer_author: author_anomalies.2,
        newcomer_committer: committer_anomalies.1,
        account_age_author,
        account_age_committer,
        repository_dormancy,
        sensitive_file: if has_sensitive { 1.0 } else { 0.0 },
        prebuilt_file: if has_prebuilt { 1.0 } else { 0.0 },
        ownership_score,
        total_score: 0.0,
    })
}
