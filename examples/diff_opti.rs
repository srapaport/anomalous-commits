use std::time::Instant;

use anyhow::Result;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use log::debug;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;
use swh_graph_stdlib::diff;

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let mut time_manual = vec![];
    let mut time_crate = vec![];
    let bar = ProgressBar::new(100);
    bar.set_style(
        ProgressStyle::with_template("{wide_bar} {pos} {percent_precise}% {elapsed_precise} {eta}")
            .unwrap(),
    );

    let mut amount_compare = 100;

    let mut revisions_found = 0;
    let mut single_parent_revisions = 0;
    let mut successful_compares = 0;

    for node in 0..graph.num_nodes() {
        if amount_compare < 1 {
            break;
        }
        if graph.properties().node_type(node) != NodeType::Revision {
            continue;
        }

        for succ in graph.successors(node) {
            match graph.properties().node_type(succ) {
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
                                debug!("Error in compute_time_diff: {:?}", e);
                            }
                        }
                    }
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
            println!("stdlib is faster than me by {} ns", time_man - time_cra);
            println!("man {} ns", time_man);
            println!("crate {} ns", time_cra);
        } else {
            println!("stdlib is slower than me by {} ns", time_cra - time_man);
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
    debug!(
        "DEBUG: compute_time_diff called with new={}, old={}",
        new, old
    );

    let new_dir = anomalous_commits::diff::get_root_dir(new, graph);
    if new_dir.is_none() {
        debug!("DEBUG: get_root_dir returned None for new commit {}", new);
        return Err(anyhow::anyhow!("Failed to get root dir for new commit"));
    }
    let new_dir = new_dir.unwrap();
    debug!("DEBUG: new_dir={}", new_dir);

    let old_dir = anomalous_commits::diff::get_root_dir(old, graph);
    if old_dir.is_none() {
        debug!("DEBUG: get_root_dir returned None for old commit {}", old);
        return Err(anyhow::anyhow!("Failed to get root dir for old commit"));
    }
    let old_dir = old_dir.unwrap();
    debug!("DEBUG: old_dir={}", old_dir);

    debug!("DEBUG: Starting manual diff computation...");
    let start = Instant::now();
    anomalous_commits::diff::compute_diff_stats(new_dir, &[old_dir], graph);
    let manual_time = start.elapsed().as_nanos();
    time_manual.push(manual_time);
    debug!("DEBUG: Manual diff took {} ns", manual_time);

    debug!("DEBUG: Starting crate diff computation...");
    let start = Instant::now();
    match diff::tree_diff_dirs(&graph, old_dir, new_dir, None) {
        Ok(_) => {
            let crate_time = start.elapsed().as_nanos();
            time_crate.push(crate_time);
            debug!("DEBUG: Crate diff took {} ns", crate_time);
        }
        Err(e) => {
            debug!("DEBUG: tree_diff_dirs failed: {:?}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
