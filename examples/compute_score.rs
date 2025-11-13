use anomalous_commits::env::RepositoryStats;
use anomalous_commits::score::{AnomalyThresholds, compute_score};
use anyhow::Result;
use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn main() -> Result<()> {
    let json_path = "repo_4959737356.json";
    let commit_swhids = vec![
        "swh:1:rev:847047cf7f81ab08352038b2204f0e7633449580",
        "swh:1:rev:6e344066a0464814a27fbd7ca8422f473956a803",
    ];

    // Load RepositoryStats from JSON file
    println!("Loading RepositoryStats from: {}", json_path);
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    let repo_stats: RepositoryStats = serde_json::from_reader(reader)?;

    // Display repository statistics
    println!("\n=== Repository Statistics ===");
    println!("Total contributors: {}", repo_stats.contributors.len());
    println!(
        "Total commit times recorded: {}",
        repo_stats.commit_time.len()
    );
    println!(
        "Total file types: {}",
        repo_stats.unique_file_type_interaction.len()
    );
    println!(
        "Total files in filesystem: {}",
        repo_stats.file_system.files.len()
    );

    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let thresholds = AnomalyThresholds::default();

    println!("\n=== Computing Anomaly Scores ===");
    for swhid_str in commit_swhids {
        println!("\n--- Commit: {} ---", swhid_str);

        let swhid = match swh_graph::SWHID::from_str(&swhid_str) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error parsing SWHID: {}", e);
                continue;
            }
        };

        let commit_node = match graph.properties().node_id(swhid) {
            Ok(node) => node,
            Err(e) => {
                eprintln!("Error finding node in graph: {}", e);
                continue;
            }
        };

        match compute_score(commit_node, &repo_stats, &thresholds, &graph) {
            Some(score) => {
                println!("Total Anomaly Score: {:.4}", score.total_score);
                println!("\nComponent Scores:");
                println!(
                    "  Time-of-Day Score:          {:.4}",
                    score.time_of_day_score
                );
                println!("  Diff Size Score:            {:.4}", score.diff_size_score);
                println!(
                    "  Contributor Experience:     {:.4}",
                    score.contributor_experience_score
                );
                println!(
                    "  File Type Diversity:        {:.4}",
                    score.file_type_diversity_score
                );
                println!(
                    "  Ownership Violation:        {:.4}",
                    score.ownership_violation_score
                );

                println!("\nAnomaly Flags:");
                println!("  Large Addition:             {}", score.is_large_addition);
                println!("  Mass Deletion:              {}", score.is_mass_deletion);
                println!(
                    "  Mass Modification:          {}",
                    score.is_mass_modification
                );
                println!("  New Contributor:            {}", score.is_new_contributor);
                println!("  Self-Commit:                {}", score.is_self_commit);
            }
            None => {
                eprintln!("Could not compute score for this commit");
            }
        }
    }

    Ok(())
}
