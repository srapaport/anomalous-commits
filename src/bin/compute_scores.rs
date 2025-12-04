use anomalous_commits::env::RepositoryStats;
use anomalous_commits::score::AnomalyWeights;
use anyhow::Result;
use dotenvy::dotenv;
use indicatif::{ProgressBar, ProgressStyle};
use rocksdb::{DB, Options};
use rusqlite::Connection;
use std::collections::HashSet;
use std::env;
use swh_graph::mph::DynMphf;
use swh_graph::{NodeType, graph::*};

fn main() -> Result<()> {
    dotenv()?;
    let graph_basename = env::var("GRAPH_BASENAME").expect("GRAPH_BASENAME not set");
    // https://github.com/Avik-Jain/100-Days-Of-ML-Code | with: 185 commits 
    let origin_url = "https://github.com/Avik-Jain/100-Days-Of-ML-Code";
    let total_revisions = 185;
    let db_repo_stats_path = "data/RepositoryStats_teaser.db";
    let db_persons_path = "data/persons_teaser.db";
    let output_db_path = "data/anomaly_scores_test.db";

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

    let mut opts = Options::default();
    opts.create_if_missing(false);
    let db_repo_stats = DB::open_for_read_only(&opts, db_repo_stats_path, false)?;

    let weights = AnomalyWeights::default();
    let repo_stats: RepositoryStats = serde_json::from_slice(
        &db_repo_stats
            .get(origin_url)?
            .expect("couldn't find origin in rocksdb"),
    )?;
    let global_stats =
        anomalous_commits::score::compute_global_account_age_statistics(db_persons_path)?;

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
        for succ in graph.successors(node) {
            match graph.properties().node_type(succ) {
                NodeType::Snapshot | NodeType::Release => {
                    to_visit.push(succ);
                },
                NodeType::Revision => {
                    if let Some(score) = anomalous_commits::score::compute_score(
                        succ,
                        &repo_stats,
                        &repo_means,
                        Some(&global_stats),
                        db_persons_path,
                        &graph,
                    ) {
                        let commit_swhid = graph.properties().swhid(succ).to_string();
                        let ownership_json = serde_json::to_string(&score.ownership_score.0)?;

                        // Insert into database
                        conn.execute(
                            "INSERT INTO anomaly_scores VALUES (
                                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
                            )",
                            rusqlite::params![
                                succ as i64,
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
