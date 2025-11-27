use anyhow::Result;
use anomalous_commits::diff::{compute_diff_stats, get_root_dir};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Write;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    println!("Searching for test cases...\n");

    let mut found_cases = FoundCases::default();

    // Create progress bar for main search
    let total_nodes = graph.num_nodes();
    let pb = ProgressBar::new(total_nodes as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} nodes ({percent}%) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    // Scan through commits to find test cases
    for node in 0..total_nodes {
        if node % 10000 == 0 {
            pb.set_position(node as u64);
            pb.set_message(format!(
                "Found: I:{} S:{} M:{} R:{} MC:{}",
                if found_cases.initial_commit {
                    "✓"
                } else {
                    "✗"
                },
                if found_cases.single_parent {
                    "✓"
                } else {
                    "✗"
                },
                if found_cases.merge_commit {
                    "✓"
                } else {
                    "✗"
                },
                if found_cases.rename_detected {
                    "✓"
                } else {
                    "✗"
                },
                if found_cases.merge_with_changes {
                    "✓"
                } else {
                    "✗"
                }
            ));
        }

        if found_cases.first_three_found() {
            pb.finish_with_message("First three test cases found!");
            break;
        }

        if graph.properties().node_type(node) != NodeType::Revision {
            continue;
        }

        let parents: Vec<usize> = graph
            .successors(node)
            .filter(|&succ| graph.properties().node_type(succ) == NodeType::Revision)
            .collect();

        // Case 1: Initial commit (no parents)
        if !found_cases.initial_commit && parents.is_empty() {
            if let Some(result) = test_commit(&graph, node, &parents, "INITIAL COMMIT (no parents)")
            {
                let mut file = File::create("case1_initial_commit.txt")?;
                writeln!(file, "{}", result)?;
                found_cases.initial_commit = true;
                pb.println("✓ Found initial commit - written to case1_initial_commit.txt");
            }
        }

        // Case 2: Single parent commit
        if !found_cases.single_parent && parents.len() == 1 {
            if let Some(result) = test_commit(&graph, node, &parents, "SINGLE PARENT COMMIT") {
                // Check if it has some changes
                if result.contains("added: ") && !result.contains("added: 0") {
                    let mut file = File::create("case2_single_parent.txt")?;
                    writeln!(file, "{}", result)?;
                    found_cases.single_parent = true;
                    pb.println("✓ Found single parent commit - written to case2_single_parent.txt");
                }
            }
        }

        // Case 3: Merge commit (multiple parents)
        if !found_cases.merge_commit && parents.len() > 1 {
            if let Some(result) =
                test_commit(&graph, node, &parents, "MERGE COMMIT (multiple parents)")
            {
                let mut file = File::create("case3_merge_commit.txt")?;
                writeln!(file, "{}", result)?;
                found_cases.merge_commit = true;
                pb.println("✓ Found merge commit - written to case3_merge_commit.txt");
            }
        }

        // Case 5: Merge commit with actual changes (multiple parents, non-zero diff)
        if !found_cases.merge_with_changes && parents.len() > 1 {
            if let Some((result, has_changes)) =
                test_commit_with_stats(&graph, node, &parents, "MERGE COMMIT WITH CHANGES")
            {
                if has_changes {
                    let mut file = File::create("case5_merge_with_changes.txt")?;
                    writeln!(file, "{}", result)?;
                    found_cases.merge_with_changes = true;
                    pb.println("✓ Found merge commit with changes - written to case5_merge_with_changes.txt");
                }
            }
        }
    }

    pb.finish_and_clear();

    // Case 4: Find a commit with renames (this requires more specific search)
    if !found_cases.rename_detected {
        println!("\nSearching for rename case...");
        let pb_rename = ProgressBar::new(1_000_000);
        pb_rename.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} nodes ({percent}%)")
                .unwrap()
                .progress_chars("=>-"),
        );

        for node in 0..graph.num_nodes().min(1_000_000) {
            if node % 1000 == 0 {
                pb_rename.set_position(node as u64);
            }

            if found_cases.rename_detected {
                pb_rename.finish_with_message("Rename case found!");
                break;
            }

            if graph.properties().node_type(node) != NodeType::Revision {
                continue;
            }

            let parents: Vec<usize> = graph
                .successors(node)
                .filter(|&succ| graph.properties().node_type(succ) == NodeType::Revision)
                .collect();

            if parents.len() >= 1 {
                if let Some(result) = test_commit(&graph, node, &parents, "RENAME DETECTED COMMIT")
                {
                    if result.contains("renamed: ") && !result.contains("renamed: 0") {
                        let mut file = File::create("case4_rename_detected.txt")?;
                        writeln!(file, "{}", result)?;
                        found_cases.rename_detected = true;
                        pb_rename.finish_and_clear();
                        println!("✓ Found rename commit - written to case4_rename_detected.txt");
                        break;
                    }
                }
            }
        }

        if !found_cases.rename_detected {
            pb_rename.finish_with_message("No rename case found in first 1M nodes");
        }
    }

    println!("\n=== SUMMARY ===");
    println!("Initial commit found: {}", found_cases.initial_commit);
    println!("Single parent commit found: {}", found_cases.single_parent);
    println!("Merge commit found: {}", found_cases.merge_commit);
    println!(
        "Rename detected commit found: {}",
        found_cases.rename_detected
    );
    println!(
        "Merge commit with changes found: {}",
        found_cases.merge_with_changes
    );
    println!(
        "\nResults written to case1_initial_commit.txt, case2_single_parent.txt, case3_merge_commit.txt, case4_rename_detected.txt, case5_merge_with_changes.txt"
    );

    Ok(())
}

#[derive(Default)]
struct FoundCases {
    initial_commit: bool,
    single_parent: bool,
    merge_commit: bool,
    rename_detected: bool,
    merge_with_changes: bool,
}

impl FoundCases {
    fn first_three_found(&self) -> bool {
        self.initial_commit && self.single_parent && self.merge_commit && self.merge_with_changes
    }
}

fn test_commit_with_stats<G: SwhFullGraph>(
    graph: &G,
    commit: usize,
    parents: &[usize],
    case_name: &str,
) -> Option<(String, bool)> {
    let output = test_commit(graph, commit, parents, case_name)?;
    
    // Check if any stats are non-zero
    let has_changes = output.contains("added: ")
        && (!output.contains("added: 0")
            || !output.contains("deleted: 0")
            || !output.contains("modified: 0")
            || !output.contains("renamed: 0"));
    
    Some((output, has_changes))
}

fn test_commit<G: SwhFullGraph>(
    graph: &G,
    commit: usize,
    parents: &[usize],
    case_name: &str,
) -> Option<String> {
    let mut output = String::new();

    // Get commit SWHID
    let commit_swhid = graph.properties().swhid(commit);

    // Use public API from src/diff.rs
    let commit_root_dir = get_root_dir(commit, graph)?;

    // Get parent directories using public API
    let parent_root_dirs: Vec<usize> = parents
        .iter()
        .filter_map(|&parent| get_root_dir(parent, graph))
        .collect();

    // Compute diff stats using public API
    let (stats, final_added, deleted_files, modified_files, all_renamed) =
        compute_diff_stats(commit_root_dir, &parent_root_dirs, graph);

    // Build renamed files pairs for sampling
    let mut renamed_files: Vec<(String, String, usize)> = Vec::new();
    if !all_renamed.is_empty() {
        for (new_path, (old_path, node_id)) in all_renamed.iter().take(10) {
            renamed_files.push((old_path.clone(), new_path.clone(), *node_id));
        }
    }

    output.push_str(&format!("\n{}\n", "=".repeat(60)));
    output.push_str(&format!("CASE: {}\n", case_name));
    output.push_str(&format!("{}\n", "=".repeat(60)));
    output.push_str(&format!("Commit SWHID: {}\n", commit_swhid));
    output.push_str(&format!("Commit node ID: {}\n", commit));
    output.push_str(&format!("Commit root dir node ID: {}\n", commit_root_dir));
    output.push_str(&format!(
        "Commit root dir SWHID: {}\n",
        graph.properties().swhid(commit_root_dir)
    ));
    output.push_str(&format!("Number of parents: {}\n", parents.len()));

    if !parents.is_empty() {
        output.push_str("Parent SWHIDs:\n");
        for (i, &parent) in parents.iter().enumerate() {
            output.push_str(&format!(
                "  Parent {}: {}\n",
                i + 1,
                graph.properties().swhid(parent)
            ));
        }
        output.push_str("Parent root dir node IDs:\n");
        for (i, &parent_dir) in parent_root_dirs.iter().enumerate() {
            output.push_str(&format!(
                "  Parent {} dir: {} (SWHID: {})\n",
                i + 1,
                parent_dir,
                graph.properties().swhid(parent_dir)
            ));
        }
    }

    output.push_str("\n--- DIFF STATS ---\n");
    output.push_str(&format!("Files added: {}\n", stats.added));
    output.push_str(&format!("Files deleted: {}\n", stats.deleted));
    output.push_str(&format!("Files modified: {}\n", stats.modified));
    output.push_str(&format!("Files renamed: {}\n", stats.renamed));

    // Show some sample files
    if !final_added.is_empty() {
        output.push_str("\nSample added files (max 10):\n");
        for (i, (path, node_id)) in final_added.iter().take(10).enumerate() {
            output.push_str(&format!("  {}: {} (node_id: {})\n", i + 1, path, node_id));
        }
        if final_added.len() > 10 {
            output.push_str(&format!("  ... and {} more\n", final_added.len() - 10));
        }
    }

    if !deleted_files.is_empty() {
        output.push_str("\nSample deleted files (max 10):\n");
        for (i, (path, node_ids)) in deleted_files.iter().take(10).enumerate() {
            output.push_str(&format!(
                "  {}: {} (node_ids: {:?})\n",
                i + 1,
                path,
                node_ids
            ));
        }
        if deleted_files.len() > 10 {
            output.push_str(&format!("  ... and {} more\n", deleted_files.len() - 10));
        }
    }

    if !modified_files.is_empty() {
        output.push_str("\nSample modified files (max 10):\n");
        for (i, (path, node_id)) in modified_files.iter().take(10).enumerate() {
            output.push_str(&format!("  {}: {} (node_id: {})\n", i + 1, path, node_id));
        }
        if modified_files.len() > 10 {
            output.push_str(&format!("  ... and {} more\n", modified_files.len() - 10));
        }
    }

    if !renamed_files.is_empty() {
        output.push_str("\nSample renamed files (max 10):\n");
        for (i, (old_path, new_path, node_id)) in renamed_files.iter().enumerate() {
            output.push_str(&format!(
                "  {}: {} -> {} (node_id: {})\n",
                i + 1,
                old_path,
                new_path,
                node_id
            ));
        }
        if stats.renamed > renamed_files.len() {
            output.push_str(&format!("  ... and {} more\n", stats.renamed - renamed_files.len()));
        }
    }

    Some(output)
}
