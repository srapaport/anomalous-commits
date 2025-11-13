use std::collections::BTreeSet;
use std::time::Instant;

use anyhow::Result;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;
use swh_graph_stdlib::diff;

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let url = "https://github.com/php/php-src";
    let swhid_url = swh_graph::SWHID::from_origin_url(url);
    let node_url = graph.properties().node_id(swhid_url)?;

    let mut time_manual = vec![];
    let mut time_crate = vec![];
    let bar = ProgressBar::new(100);
    bar.set_style(
        ProgressStyle::with_template("{wide_bar} {pos} {percent_precise}% {elapsed_precise} {eta}")
            .unwrap(),
    );

    let mut amount_compare = 100;

    let mut to_visit = vec![node_url];
    let mut visited = BTreeSet::new();
    let mut revisions_found = 0;
    let mut single_parent_revisions = 0;
    let mut successful_compares = 0;

    while let Some(visiting) = to_visit.pop()
        && amount_compare > 0
    {
        if visited.contains(&visiting) {
            continue;
        }
        visited.insert(visiting);

        for succ in graph.successors(visiting) {
            match graph.properties().node_type(succ) {
                NodeType::Snapshot | NodeType::Release => {
                    to_visit.push(succ);
                }
                NodeType::Revision => {
                    revisions_found += 1;
                    let mut parents = vec![];
                    for p_succ in graph.successors(succ) {
                        if graph.properties().node_type(succ) == NodeType::Revision {
                            parents.push(p_succ);
                        }
                    }
                    if parents.len() == 1 {
                        single_parent_revisions += 1;
                        match compute_time_diff(
                            succ,
                            *parents.get(0).unwrap(),
                            &graph,
                            &mut time_manual,
                            &mut time_crate,
                        ) {
                            Ok(_) => {
                                bar.inc(1);
                                amount_compare -= 1;
                                successful_compares += 1;
                            }
                            Err(e) => {
                                println!("Error in compute_time_diff: {:?}", e);
                            }
                        }
                    }
                    to_visit.push(succ);
                }
                _ => (),
            }
        }
    }

    println!("\n=== Debug Statistics ===");
    println!("Total revisions found: {}", revisions_found);
    println!("Single parent revisions: {}", single_parent_revisions);
    println!("Successful comparisons: {}", successful_compares);
    println!("time_manual length: {}", time_manual.len());
    println!("time_crate length: {}", time_crate.len());

    for i in 0..time_crate.len() {
        let time_man = time_manual.get(i).unwrap();
        let time_cra = time_crate.get(i).unwrap();
        if time_cra < time_man {
            println!("stdlib is faster than me by {}ms", time_man - time_cra);
        } else {
            println!("stdlib is slower than me by {}ms", time_cra - time_man);
        }
    }
    Ok(())
}

fn compute_time_diff<G: SwhFullGraph>(
    new: usize,
    old: usize,
    graph: &G,
    time_manual: &mut Vec<u128>,
    time_crate: &mut Vec<u128>,
) -> Result<()> {
    println!(
        "DEBUG: compute_time_diff called with new={}, old={}",
        new, old
    );

    let new_dir = anomalous_commits::diff::get_root_dir(new, graph);
    if new_dir.is_none() {
        println!("DEBUG: get_root_dir returned None for new commit {}", new);
        return Err(anyhow::anyhow!("Failed to get root dir for new commit"));
    }
    let new_dir = new_dir.unwrap();
    println!("DEBUG: new_dir={}", new_dir);

    let old_dir = anomalous_commits::diff::get_root_dir(old, graph);
    if old_dir.is_none() {
        println!("DEBUG: get_root_dir returned None for old commit {}", old);
        return Err(anyhow::anyhow!("Failed to get root dir for old commit"));
    }
    let old_dir = old_dir.unwrap();
    println!("DEBUG: old_dir={}", old_dir);

    println!("DEBUG: Starting manual diff computation...");
    let start = Instant::now();
    anomalous_commits::diff::compute_diff_stats(new_dir, &[old_dir], graph);
    let manual_time = start.elapsed().as_millis();
    time_manual.push(manual_time);
    println!("DEBUG: Manual diff took {}ms", manual_time);

    println!("DEBUG: Starting crate diff computation...");
    let start = Instant::now();
    match diff::tree_diff_dirs(&graph, old_dir, new_dir, None) {
        Ok(_) => {
            let crate_time = start.elapsed().as_millis();
            time_crate.push(crate_time);
            println!("DEBUG: Crate diff took {}ms", crate_time);
        }
        Err(e) => {
            println!("DEBUG: tree_diff_dirs failed: {:?}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
