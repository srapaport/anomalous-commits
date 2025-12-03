use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::time::Instant;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use swh_graph::mph::DynMphf;
use swh_graph::{NodeType, graph::*};

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new(
        "/home/infres/rapaport/datasets/2024-08-23-popular-500-python/compressed/graph",
    )?
    .load_all_properties::<DynMphf>()?
    .load_forward_labels()?
    .load_backward_labels()?;

    // We want to know the look of a plot showing
    // the amount of commits per contributor for a given project
    let mut count_commits = HashMap::new();
    // let url = "https://github.com/RIAEvangelist/node-ipc";
    // let url = "https://github.com/uutils/coreutils/";
    // let swhid_url = swh_graph::SWHID::from_origin_url(url);
    // let node_url = graph.properties().node_id(swhid_url)?;

    let start_time = Instant::now();
    let pb_ori =
        ProgressBar::new(*graph.num_nodes_by_type()?.get(&NodeType::Origin).unwrap() as u64);
    pb_ori.set_style(
        ProgressStyle::default_bar()
            .template("{bar:40.cyan/blue} {pos}/{len} [{elapsed_precise}] {msg}")
            .unwrap()
            .progress_chars("##-"),
    );
    // let pb = ProgressBar::new_spinner();
    // pb.set_style(
    //     ProgressStyle::default_spinner()
    //         .template("{spinner:.green} [{elapsed_precise}] {msg}")
    //         .unwrap(),
    // );

    (0..graph.num_nodes())
        .into_iter()
        .filter(|node| graph.properties().node_type(*node) == NodeType::Origin)
        .for_each(|ori| {
            // let mut commits_parsed = 0u64;

            let mut visited = HashSet::new();
            let mut to_visit = vec![ori];
            while let Some(node) = to_visit.pop() {
                if visited.contains(&node) {
                    continue;
                }
                visited.insert(node);

                for succ in graph.successors(node) {
                    match graph.properties().node_type(succ) {
                        NodeType::Release | NodeType::Snapshot => {
                            to_visit.push(succ);
                        }
                        NodeType::Revision => {
                            if let Some(author_hash) = graph.properties().author_id(succ) {
                                let author_id = format!("author:{:064x}", author_hash);
                                *count_commits
                                    .entry(ori)
                                    .or_insert_with(HashMap::new)
                                    .entry(author_id)
                                    .or_insert(0) += 1;

                                // commits_parsed += 1;
                                // if commits_parsed % 1000 == 0 {
                                //     pb.set_message(format!("Commits parsed: {}", commits_parsed));
                                // }
                                to_visit.push(succ);
                            }
                        }
                        _ => (),
                    }
                }
            }
            pb_ori.inc(1);
        });
    // Write count_commits to JSON file
    let json_output = serde_json::to_string_pretty(&count_commits)?;
    let mut file = File::create("analysis/contributors_commits_all.json")?;
    file.write_all(json_output.as_bytes())?;

    let elapsed = start_time.elapsed();
    pb_ori.finish_with_message(format!("Completed in {:.2}s", elapsed.as_secs_f64()));
    // pb.finish_with_message(format!("Completed in {:.2}s", elapsed.as_secs_f64()));

    Ok(())
}
