use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::str::FromStr;
use std::time::Instant;

use anyhow::Result;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn date_to_timestamp(year: i32, month: u32, day: u32) -> i64 {
    use chrono::{NaiveDate, TimeZone, Utc};
    let naive_date = NaiveDate::from_ymd_opt(year, month, day).expect("Invalid date");
    Utc.from_utc_datetime(&naive_date.and_hms_opt(0, 0, 0).unwrap())
        .timestamp()
}

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let start = Instant::now();
    //let url = "https://github.com/php/php-src";
    let url = "https://github.com/RIAEvangelist/node-ipc";
    let swhid_url = swh_graph::SWHID::from_origin_url(url);
    let node_url = graph.properties().node_id(swhid_url)?;
    single_repo(node_url, &graph)?;
    // find_sus(
    //     url,
    //     &[
    //         "swh:1:rev:847047cf7f81ab08352038b2204f0e7633449580",
    //         "swh:1:rev:6e344066a0464814a27fbd7ca8422f473956a803"
    //     ],
    //     &graph,
    // )?;
    println!("elapsed: {:?}", start.elapsed());
    Ok(())
}

fn find_sus<G: SwhFullGraph>(url: &str, sus_commits: &[&str], graph: &G) -> Result<()> {
    let swhid_url = swh_graph::SWHID::from_origin_url(url);
    let node_url = graph.properties().node_id(swhid_url)?;
    // let snapshot = anomalous_commits::outliers::retrieve_latest_snapshots(node_url, graph)
    //     .ok_or_else(|| anyhow::anyhow!("No snapshot found for repository {}", url))?;

    let sus_nodes = sus_commits
        .iter()
        .filter_map(|commit| {
            let swhid = swh_graph::SWHID::from_str(*commit).and_then(|swhid|{
                println!("sus commit is in the graph");
                Ok(swhid)
            }).ok()?;
            graph.properties().node_id(swhid).ok()
        })
        .collect::<Vec<usize>>();

    let mut visited = BTreeSet::new();
    let mut to_visit = vec![node_url];
    while let Some(visiting) = to_visit.pop() {
        if visited.contains(&visiting) {
            continue;
        }
        visited.insert(visiting);

        for succ in graph.successors(visiting) {
            match graph.properties().node_type(succ) {
                NodeType::Revision => {
                    if sus_nodes.contains(&succ) {
                        println!("Found sus!");
                        return Ok(());
                    }
                    if !visited.contains(&succ) {
                        to_visit.push(succ);
                    }
                }
                NodeType::Release | NodeType::Snapshot => {
                    if !visited.contains(&succ) {
                        to_visit.push(succ);
                    }
                }
                _ => (),
            }
        }
    }
    println!("didn't find sus...");
    Ok(())
}

fn single_repo<G: SwhFullGraph>(repo: usize, graph: &G) -> Result<()> {
    let snapshot = anomalous_commits::outliers::retrieve_latest_snapshots(repo, graph)
        .ok_or_else(|| anyhow::anyhow!("No snapshot found for repository {}", repo))?;

    let mut repo_stats = process_snapshot(snapshot, graph);

    // Sort commit times before serialization
    repo_stats.commit_time.sort_unstable();

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&repo_stats)?;

    // Write to file
    let filename = format!("repo_{}.json", repo);
    let mut file = File::create(&filename)?;
    file.write_all(json.as_bytes())?;

    println!("Repository data flushed to {}", filename);

    Ok(())
}

fn process_snapshot<G: SwhFullGraph>(
    snapshot: usize,
    graph: &G,
) -> anomalous_commits::env::RepositoryStats {
    let props = graph.properties();
    let mut repo = anomalous_commits::env::RepositoryStats::new();
    let mut visited = BTreeSet::new();
    let bar = ProgressBar::new(0);
    bar.set_style(ProgressStyle::with_template("{msg} {pos} {elapsed_precise}").unwrap());
    bar.set_message("building repo stats");
    for succ in graph.successors(snapshot) {
        // We only look at commits, we skip releases
        // We don't retrieve the information on branch name so far
        if props.node_type(succ) != NodeType::Revision {
            continue;
        }
        bar.inc(1);
        process_revision(succ, &mut repo, &mut visited, graph, &bar, None);
    }
    repo
}

fn process_revision<G: SwhFullGraph>(
    commit: usize,
    repo: &mut anomalous_commits::env::RepositoryStats,
    visited: &mut BTreeSet<usize>,
    graph: &G,
    bar: &ProgressBar,
    timestamp_limit: Option<i64>,
) {
    let mut stack = vec![commit];

    while let Some(current_commit) = stack.pop() {
        if visited.contains(&current_commit) {
            continue;
        }
        visited.insert(current_commit);

        anomalous_commits::outliers::update_contributors(current_commit, repo, graph);
        let props = graph.properties();

        // update time between commits
        let mut parents = vec![];
        for succ in graph.successors(current_commit) {
            if props.node_type(succ) != NodeType::Revision {
                continue;
            }
            parents.push(succ);
            if let Some(commit_date) = props.committer_timestamp(current_commit) {
                repo.update_commit_time(commit_date);
            }
            // Add successor to stack if not visited
            if !visited.contains(&succ) {
                if let Some(commit_date) = props.committer_timestamp(current_commit) {
                    if let Some(timestamp_limit) = timestamp_limit
                        && commit_date < timestamp_limit
                    {
                        continue;
                    }
                }
                bar.inc(1);
                stack.push(succ);
            }
        }
        anomalous_commits::diff::compute_diff(current_commit, parents, repo, graph);
    }
}
