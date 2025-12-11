use anomalous_commits::env::RepositoryStats;
use anomalous_commits::score::AnomalyWeights;
use anyhow::Result;
use dotenvy::dotenv;
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::Connection;
use std::collections::HashSet;
use std::env;
use std::fs;
use swh_graph::mph::DynMphf;
use swh_graph::{NodeType, graph::*};

fn main() -> Result<()> {
    dotenv()?;
    let graph_basename = env::var("GRAPH_BASENAME").expect("GRAPH_BASENAME not set");
    let origin_url = "https://github.com/RIAEvangelist/node-ipc";
    let total_revisions = 491;
    // let db_repo_stats_path = "data/RepositoryStats_teaser.db";
    // let db_persons_path = "data/persons_teaser.db";
    let output_db_path = "data/anomaly_scores_validation.db";

    // Create SQLite database and table
    let conn = Connection::open(output_db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS anomaly_scores (
            commit_node INTEGER PRIMARY KEY,
            commit_swhid TEXT,
            large_addition REAL,
            large_deletion REAL,
            large_modification REAL,
            large_moves REAL,
            time_of_day REAL,
            author_tod REAL,
            author_tod_global REAL,
            committer_tod REAL,
            committer_tod_global REAL,
            file_type_interaction REAL,
            file_type_interaction_author REAL,
            number_commits_author REAL,
            number_commits_committer REAL,
            newcomer_author REAL,
            newcomer_committer REAL,
            account_age_author REAL,
            account_age_committer REAL,
            repository_dormancy REAL,
            sensitive_file REAL,
            prebuilt_file REAL,
            ownership_score_json TEXT,
            ownership_has_unknown INTEGER,
            total_score REAL,
            weighted_score REAL
        )",
        [],
    )?;

    let weights = AnomalyWeights::default();

    // Load repository stats from JSON file
    let json_path = "repo_4959737356.json";
    let json_content = fs::read_to_string(json_path)?;
    let repo_stats: RepositoryStats = serde_json::from_str(&json_content)?;
    // let global_stats =
    //     anomalous_commits::score::compute_global_account_age_statistics(db_persons_path)?;

    // Build time-of-day profiles once (optimization)
    println!("Building time-of-day profiles...");
    let repo_tod_profile = anomalous_commits::score::build_repo_tod_profile(&repo_stats);
    let person_tod_profiles = anomalous_commits::score::build_person_tod_profiles(&repo_stats);

    // let mut opts_global = Options::default();
    // opts_global.create_if_missing(false);
    // let Ok(db_global) = DB::open_for_read_only(&opts_global, db_persons_path, false) else {
    //     log::warn!("failed to open database {}", db_persons_path);
    //     return Err(anyhow!("failed to open persons database"));
    // };

    // let global_person_tod_profiles = anomalous_commits::score::build_global_person_tod_profiles(&db_global);
    // println!("Built {} person ToD profiles, {} global person ToD profiles",
    //          person_tod_profiles.len(), global_person_tod_profiles.len());

    // Drop the global DB connection as it's no longer needed
    // drop(db_global);

    let repo_means = anomalous_commits::score::compute_repo_statistics(&repo_stats);
    let graph = SwhBidirectionalGraph::new(graph_basename)?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let swhid_url = swh_graph::SWHID::from_origin_url(origin_url);
    let node_url = graph.properties().node_id(swhid_url)?;

    let pb = ProgressBar::new(total_revisions);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("Failed to set progress bar template")
            .progress_chars("#>-")
    );
    pb.set_message("Computing anomaly scores...");

    let mut visited = HashSet::new();
    let mut to_visit = vec![node_url];

    let mut insert_count = 0;
    while let Some(node) = to_visit.pop() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node);
        if graph.properties().node_type(node) == NodeType::Revision {
            if let Some(score) = anomalous_commits::score::compute_score(
                node,
                &repo_stats,
                &repo_means,
                None,
                &graph,
                repo_tod_profile.as_ref(),
                &person_tod_profiles,
                None,
            ) {
                let commit_swhid = graph.properties().swhid(node).to_string();
                let ownership_json = serde_json::to_string(&score.ownership_score.0)?;

                // Insert into database
                conn.execute(
                    "INSERT INTO anomaly_scores VALUES (
                                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
                            )",
                    rusqlite::params![
                        node as i64,
                        commit_swhid,
                        score.large_addition,
                        score.large_deletion,
                        score.large_modification,
                        score.large_moves,
                        score.time_of_day,
                        score.author_tod,
                        score.author_tod_global,
                        score.committer_tod,
                        score.committer_tod_global,
                        score.file_type_interaction,
                        score.file_type_interaction_author,
                        score.number_commits_author,
                        score.number_commits_committer,
                        score.newcomer_author,
                        score.newcomer_committer,
                        score.account_age_author,
                        score.account_age_committer,
                        score.repository_dormancy,
                        score.sensitive_file,
                        score.prebuilt_file,
                        ownership_json,
                        score.ownership_score.1 as i64,
                        score.total_score,
                        anomalous_commits::score::compute_weighted_score(&score, &weights),
                    ],
                )?;

                insert_count += 1;
                pb.inc(1);
            } else {
                pb.inc(1);
            }
        }
        for succ in graph.successors(node) {
            match graph.properties().node_type(succ) {
                NodeType::Snapshot | NodeType::Release | NodeType::Revision => {
                    if visited.contains(&succ) {
                        continue;
                    }
                    to_visit.push(succ);
                }
                _ => (),
            }
        }
    }

    pb.finish_with_message("Completed!");
    println!("Total commits inserted: {}", insert_count);
    Ok(())
}
