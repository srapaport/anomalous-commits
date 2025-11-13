use swh_graph::NodeType;
use swh_graph::graph::SwhFullGraph;

use crate::env::RepositoryStats;
use crate::time_of_day::{Commit, TimeOfDayProfile};

/// Thresholds for anomaly detection
#[derive(Debug, Clone)]
pub struct AnomalyThresholds {
    /// Minimum ratio of files added to trigger large_addition flag (e.g., 0.1 = 10% of repo)
    pub large_addition_ratio: f64,
    /// Minimum ratio of files deleted to trigger mass_deletion flag (e.g., 0.1 = 10% of repo)
    pub mass_deletion_ratio: f64,
    /// Minimum ratio of files modified to trigger mass_modification flag (e.g., 0.2 = 20% of repo)
    pub mass_modification_ratio: f64,
    /// Threshold for new contributor (commits < this value)
    pub new_contributor_threshold: usize,
    /// Weight for time-of-day anomaly score
    pub tod_weight: f64,
    /// Weight for contributor experience
    pub contributor_weight: f64,
    /// Weight for file type diversity
    pub file_type_weight: f64,
    /// Weight for ownership violation
    pub ownership_weight: f64,
}

impl Default for AnomalyThresholds {
    fn default() -> Self {
        Self {
            large_addition_ratio: 0.1,
            mass_deletion_ratio: 0.1,
            mass_modification_ratio: 0.2,
            new_contributor_threshold: 10,
            tod_weight: 1.0,
            contributor_weight: 1.2,
            file_type_weight: 0.8,
            ownership_weight: 1.3,
        }
    }
}

/// Detailed commit information for scoring
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub timestamp: i64,
    pub tz_offset: i16,
    pub author_id: String,
    pub committer_id: String,
    pub files_added: usize,
    pub files_deleted: usize,
    pub files_modified: usize,
    pub files_renamed: usize,
    // pub file_extensions: HashMap<String, usize>, // Extension -> count of interactions
}

impl CommitInfo {
    pub fn new<G: SwhFullGraph>(commit: usize, graph: &G) -> Option<Self> {
        let props = graph.properties();
        let timestamp = props.committer_timestamp(commit)?;
        let tz_offset = props.committer_timestamp_offset(commit).unwrap_or(0);
        let author_bytes = props.author_id(commit)?;
        let author_id = format!("author:{:x}", author_bytes);
        let committer_bytes = props.committer_id(commit)?;
        let committer_id = format!("committer:{:x}", committer_bytes);

        let root_dir = crate::diff::get_root_dir(commit, graph)?;
        let mut parents = vec![];
        for succ in graph.successors(commit) {
            if props.node_type(succ) != NodeType::Revision {
                continue;
            }
            parents.push(succ);
        }
        let (stats, _, _, _, _) = crate::diff::compute_diff_stats(
            root_dir,
            &parents
                .into_iter()
                .filter_map(|parent| crate::diff::get_root_dir(parent, graph))
                .collect::<Vec<usize>>(),
            graph,
        );
        Some(Self {
            timestamp,
            tz_offset,
            author_id,
            committer_id,
            files_added: stats.added,
            files_deleted: stats.deleted,
            files_modified: stats.deleted,
            files_renamed: stats.renamed,
            // file_extensions: (),
        })
    }
}

/// Anomaly score breakdown
#[derive(Debug, Clone)]
pub struct AnomalyScore {
    /// Overall anomaly score (0.0 to 1.0, higher = more anomalous)
    pub total_score: f64,
    /// Individual component scores
    pub time_of_day_score: f64,
    pub diff_size_score: f64,
    pub contributor_experience_score: f64,
    pub file_type_diversity_score: f64,
    pub ownership_violation_score: f64,
    /// Boolean flags for specific anomalies
    pub is_large_addition: bool,
    pub is_mass_deletion: bool,
    pub is_mass_modification: bool,
    pub is_new_contributor: bool,
    pub is_self_commit: bool,
}

impl AnomalyScore {
    /// Create a new score with default values
    pub fn new() -> Self {
        Self {
            total_score: 0.0,
            time_of_day_score: 0.0,
            diff_size_score: 0.0,
            contributor_experience_score: 0.0,
            file_type_diversity_score: 0.0,
            ownership_violation_score: 0.0,
            is_large_addition: false,
            is_mass_deletion: false,
            is_mass_modification: false,
            is_new_contributor: false,
            is_self_commit: false,
        }
    }
}

/// Main function to compute anomaly score for a commit
///
/// # Arguments
/// * `commit_info` - Information about the commit being scored
/// * `repo_stats` - Statistics about the repository
/// * `thresholds` - Thresholds for anomaly detection
///
/// # Returns
/// An `AnomalyScore` struct containing the overall score and component breakdowns
pub fn compute_score<G: SwhFullGraph>(
    commit: usize,
    repo_stats: &RepositoryStats,
    thresholds: &AnomalyThresholds,
    graph: &G,
) -> Option<AnomalyScore> {
    let commit_info = CommitInfo::new(commit, graph)?;

    let mut score = AnomalyScore::new();

    // 1. Time-of-Day Anomaly Score
    score.time_of_day_score = compute_time_of_day_score(&commit_info, repo_stats);

    // 2. Diff Size Anomaly Score
    let (diff_score, large_add, mass_del, mass_mod) =
        compute_diff_size_score(&commit_info, repo_stats, thresholds);
    score.diff_size_score = diff_score;
    score.is_large_addition = large_add;
    score.is_mass_deletion = mass_del;
    score.is_mass_modification = mass_mod;

    // 3. Contributor Experience Score
    let (contrib_score, is_new) =
        compute_contributor_experience_score(&commit_info, repo_stats, thresholds);
    score.contributor_experience_score = contrib_score;
    score.is_new_contributor = is_new;

    // 4. File Type Diversity Score
    // score.file_type_diversity_score = compute_file_type_diversity_score(&commit_info, repo_stats);

    // 5. Ownership Violation Score
    // score.ownership_violation_score = compute_ownership_violation_score(&commit_info, repo_stats);

    // Check if self-committed
    score.is_self_commit = commit_info.author_id == commit_info.committer_id;

    // Calculate weighted total score
    score.total_score = calculate_total_score(&score, thresholds);

    Some(score)
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

/// Calculate diff size anomaly score based on repository patterns
fn compute_diff_size_score(
    commit_info: &CommitInfo,
    repo_stats: &RepositoryStats,
    thresholds: &AnomalyThresholds,
) -> (f64, bool, bool, bool) {
    let total_files = repo_stats.file_system.files.len() as f64;
    if total_files == 0.0 {
        return (0.0, false, false, false);
    }

    let added_ratio = commit_info.files_added as f64 / total_files;
    let deleted_ratio = commit_info.files_deleted as f64 / total_files;
    let modified_ratio = commit_info.files_modified as f64 / total_files;

    let is_large_addition = added_ratio > thresholds.large_addition_ratio;
    let is_mass_deletion = deleted_ratio > thresholds.mass_deletion_ratio;
    let is_mass_modification = modified_ratio > thresholds.mass_modification_ratio;

    // Calculate repository-wide statistics for normalization
    let avg_added = repo_stats.file_system.files_added as f64 / total_files.max(1.0);
    let avg_modified = repo_stats.file_system.files_modified as f64 / total_files.max(1.0);

    // Normalized scores
    let add_score = if avg_added > 0.0 {
        (added_ratio / avg_added).min(1.0)
    } else {
        added_ratio.min(1.0)
    };

    let mod_score = if avg_modified > 0.0 {
        (modified_ratio / avg_modified).min(1.0)
    } else {
        modified_ratio.min(1.0)
    };

    // Deletions are generally more suspicious
    let del_score = (deleted_ratio * 2.0).min(1.0);

    // Combined diff size score
    let diff_score = (add_score + mod_score + del_score) / 3.0;

    (
        diff_score,
        is_large_addition,
        is_mass_deletion,
        is_mass_modification,
    )
}

/// Calculate contributor experience score (higher = less experienced = more anomalous)
fn compute_contributor_experience_score(
    commit_info: &CommitInfo,
    repo_stats: &RepositoryStats,
    thresholds: &AnomalyThresholds,
) -> (f64, bool) {
    let author_record = repo_stats.contributors.get(&commit_info.author_id);

    let (commits_count, time_since_first) = if let Some(record) = author_record {
        (record.number_of_commits, record.time_since_first_commit)
    } else {
        // Unknown author - very suspicious
        return (1.0, true);
    };

    let is_new = commits_count < thresholds.new_contributor_threshold;

    // Experience score based on number of commits (inverse logarithmic scale)
    let commit_score = if commits_count == 0 {
        1.0
    } else {
        let normalized = (commits_count as f64).log10() / 4.0; // log10(10000) = 4
        1.0 - normalized.min(1.0)
    };

    // Time-based experience (inverse - less time = less trusted)
    const SECONDS_IN_YEAR: f64 = 365.25 * 24.0 * 3600.0;
    let time_score = if time_since_first == 0 {
        1.0
    } else {
        let years = time_since_first as f64 / SECONDS_IN_YEAR;
        let normalized = years / 5.0; // 5 years = max experience
        1.0 - normalized.min(1.0)
    };

    // Weighted combination
    let experience_score = (commit_score * 0.6 + time_score * 0.4).min(1.0);

    (experience_score, is_new)
}

/// Calculate file type diversity score (touching many different file types is more anomalous)
// fn compute_file_type_diversity_score(
//     commit_info: &CommitInfo,
//     repo_stats: &RepositoryStats,
// ) -> f64 {
//     if commit_info.file_extensions.is_empty() {
//         return 0.0;
//     }

//     let commit_types = commit_info.file_extensions.len();
//     let repo_types = repo_stats.unique_file_type_interaction.len();

//     if repo_types == 0 {
//         return 0.0;
//     }

//     // Score based on proportion of file types touched
//     let diversity_ratio = commit_types as f64 / repo_types as f64;

//     // Non-linear scaling: touching many types is increasingly suspicious
//     diversity_ratio.powf(1.5).min(1.0)
// }

/// Calculate ownership violation score (modifying files you didn't create/rarely touch)
// fn compute_ownership_violation_score(
//     commit_info: &CommitInfo,
//     repo_stats: &RepositoryStats,
// ) -> f64 {
//     if commit_info.file_extensions.is_empty() {
//         return 0.0;
//     }

//     let author_id = &commit_info.author_id;
//     let mut violation_score = 0.0;
//     let mut scored_files = 0;

//     // Check ownership for each file type
//     for (extension, _count) in &commit_info.file_extensions {
//         // Find files with this extension
//         for (path, file_info) in &repo_stats.file_system.files {
//             if !path.ends_with(extension) {
//                 continue;
//             }

//             scored_files += 1;

//             // Check if author has interacted with this file
//             let interaction_count = file_info.interactions.get(author_id).unwrap_or(&0);
//             let total_interactions: usize = file_info.interactions.values().sum();

//             if total_interactions == 0 {
//                 continue;
//             }

//             // Calculate author's ownership percentage
//             let ownership_pct = *interaction_count as f64 / total_interactions as f64;

//             // Low ownership = high violation
//             violation_score += 1.0 - ownership_pct;
//         }
//     }

//     if scored_files == 0 {
//         return 0.0;
//     }

//     (violation_score / scored_files as f64).min(1.0)
// }


/// Calculate weighted total anomaly score
fn calculate_total_score(score: &AnomalyScore, thresholds: &AnomalyThresholds) -> f64 {
    let weighted_sum = score.time_of_day_score * thresholds.tod_weight
        + score.contributor_experience_score * thresholds.contributor_weight;
        // + score.file_type_diversity_score * thresholds.file_type_weight
        // + score.ownership_violation_score * thresholds.ownership_weight

    let total_weight = thresholds.tod_weight
        + thresholds.contributor_weight;
        //+ thresholds.file_type_weight
        //+ thresholds.ownership_weight;

    // Normalize to 0-1 range
    let normalized = weighted_sum / total_weight;

    // Apply bonus for boolean flags
    let mut bonus = 0.0;
    if score.is_large_addition {
        bonus += 0.05;
    }
    if score.is_mass_deletion {
        bonus += 0.1; // Deletions are more suspicious
    }
    if score.is_mass_modification {
        bonus += 0.05;
    }
    if score.is_new_contributor {
        bonus += 0.03;
    }
    if score.is_self_commit {
        bonus += 0.02; // self commits slightly more suspicious
    }

    (normalized + bonus).min(1.0)
}
