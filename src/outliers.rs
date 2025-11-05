use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use num_cpus;
use rocksdb::{DB, Options};
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::labels::{EdgeLabel, VisitStatus};
use swh_graph::mph::DynMphf;

use crate::diff;
use crate::env;

// Error counters for defensive programming
static AUTHOR_ID_FAILURES: AtomicUsize = AtomicUsize::new(0);
static COMMITTER_ID_FAILURES: AtomicUsize = AtomicUsize::new(0);
static AUTHOR_TIMESTAMP_FAILURES: AtomicUsize = AtomicUsize::new(0);
static COMMITTER_TIMESTAMP_FAILURES: AtomicUsize = AtomicUsize::new(0);
static AUTHOR_TIMEZONE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static COMMITTER_TIMEZONE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static URL_ERRORS: AtomicUsize = AtomicUsize::new(0);
static URL_ERRORS_UTF8: AtomicUsize = AtomicUsize::new(0);

pub fn retrieve_latest_snapshots<G: SwhFullGraph>(origin: usize, graph: &G) -> Option<usize> {
    let props = graph.properties();
    let mut snapshots = vec![];
    for (succ, labels) in graph.labeled_successors(origin) {
        // We only retrieve snapshots
        if props.node_type(succ) != NodeType::Snapshot {
            continue;
        }
        // We retrieve all snapshots
        for label in labels {
            if let EdgeLabel::Visit(v) = label {
                // We only retrieve Full visits
                if v.status() == VisitStatus::Partial {
                    break;
                }
                snapshots.push((succ, v.timestamp()));
            }
        }
    }
    if !snapshots.is_empty() {
        snapshots.sort_by_key(|s| s.1);
        return Some(snapshots.last().unwrap().0);
    }

    None
}

pub fn parsing_graph_repositories(
    graph_basename: String,
    db_repo_stats_path: String,
) -> Result<()> {
    let graph = SwhBidirectionalGraph::new(graph_basename)?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;
    let props = graph.properties();

    if std::path::Path::new(&db_repo_stats_path).exists() {
        DB::destroy(&Options::default(), &db_repo_stats_path)?;
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_cpus::get() / 3)
        .build()?;

    pool.install(|| {
        rayon::scope(|s| {
            for node in 0..graph.num_nodes() {
                // We only look from an origin node
                if props.node_type(node) != NodeType::Origin {
                    continue;
                }
                let Some(url) = props.message(node) else {
                    URL_ERRORS.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Ok(url_string) = String::from_utf8(url) else {
                    URL_ERRORS_UTF8.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                // We retrieve the latest snapshot of the origin
                let Some(latest_snapshot) = retrieve_latest_snapshots(node, &graph) else {
                    continue;
                };

                // We process the latest snapshot in a thread
                let graph = &graph;
                let db_repo_stats_path = &db_repo_stats_path;
                s.spawn(move |_| {
                    let mut repo = process_snapshot(latest_snapshot, graph);
                    repo.flush_to_rocksdb(url_string, db_repo_stats_path)
                        .unwrap();
                });
            }
        });
    });

    // Print error statistics after processing
    print_error_stats();

    Ok(())
}

/// #Files added [x]
/// #Files removed [x]
/// #Files modified [x]
/// #Files renamed [x]
/// #Unique file types modified per repo [ ]
/// Proportion of ownership [ ]
/// Number of commits for an author in a repo [x]
/// Time since 1st commit of an author in a repo [x]
/// Commit-time distribution per repo [x]
/// Time between commits [x]
/// Time between commits to a file [ ]
/// File changed by many dev [ ]
/// Introduction of prebuilt binaries [ ]
/// Type of files changed per dev Xrepo [ ]
pub fn process_snapshot<G: SwhFullGraph>(snapshot: usize, graph: &G) -> env::RepositoryStats {
    let props = graph.properties();
    let mut repo = env::RepositoryStats::new();
    let mut visited = BTreeSet::new();
    for succ in graph.successors(snapshot) {
        // We only look at commits, we skip releases
        // We don't retrieve the information on branch name so far
        if props.node_type(succ) != NodeType::Revision {
            continue;
        }
        process_revision(succ, &mut repo, &mut visited, graph);
    }
    repo
}

fn process_revision<G: SwhFullGraph>(
    commit: usize,
    repo: &mut env::RepositoryStats,
    visited: &mut BTreeSet<usize>,
    graph: &G,
) {
    let mut stack = vec![commit];
    
    while let Some(current_commit) = stack.pop() {
        if visited.contains(&current_commit) {
            continue;
        }
        visited.insert(current_commit);
        
        update_contributors(current_commit, repo, graph);
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
                stack.push(succ);
            }
        }
        diff::compute_diff(current_commit, parents, repo, graph);
    }
}

fn update_contributors<G: SwhFullGraph>(commit: usize, repo: &mut env::RepositoryStats, graph: &G) {
    let props = graph.properties();

    // Defensive programming: handle potential failures
    let Some(author) = props.author_id(commit) else {
        AUTHOR_ID_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let Some(committer) = props.committer_id(commit) else {
        COMMITTER_ID_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let Some(date) = props.author_timestamp(commit) else {
        AUTHOR_TIMESTAMP_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let Some(cdate) = props.committer_timestamp(commit) else {
        COMMITTER_TIMESTAMP_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let Some(tz) = props.author_timestamp_offset(commit) else {
        AUTHOR_TIMEZONE_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let Some(ctz) = props.committer_timestamp_offset(commit) else {
        COMMITTER_TIMEZONE_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let author_id = format!("author:{:x}", author);
    let committer_id = format!("committer:{:x}", committer);
    let self_committed = author == committer;

    let commit_id = props.swhid(commit).hash;
    let commit_hash = commit_id
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    repo.update_contributors(author_id, &commit_hash, date, tz, self_committed);
    repo.update_contributors(committer_id, &commit_hash, cdate, ctz, self_committed);
}

/// Print error statistics
pub fn print_error_stats() {
    let author_id_fails = AUTHOR_ID_FAILURES.load(Ordering::Relaxed);
    let committer_id_fails = COMMITTER_ID_FAILURES.load(Ordering::Relaxed);
    let author_ts_fails = AUTHOR_TIMESTAMP_FAILURES.load(Ordering::Relaxed);
    let committer_ts_fails = COMMITTER_TIMESTAMP_FAILURES.load(Ordering::Relaxed);

    println!("\n=== Error Statistics ===");
    println!("Author ID failures: {}", author_id_fails);
    println!("Committer ID failures: {}", committer_id_fails);
    println!("Author timestamp failures: {}", author_ts_fails);
    println!("Committer timestamp failures: {}", committer_ts_fails);
    println!(
        "Total failures: {}",
        author_id_fails + committer_id_fails + author_ts_fails + committer_ts_fails
    );

    let author_id_fails_diff = diff::AUTHOR_ID_FAILURES.load(Ordering::Relaxed);
    let author_ts_fails_diff = diff::AUTHOR_TIMESTAMP_FAILURES.load(Ordering::Relaxed);
    let root_dir_err = diff::ROOT_DIR_ERR.load(Ordering::Relaxed);
    let path_name_err = diff::NAME_UTF8_ERR.load(Ordering::Relaxed);

    println!("\n=== Diff Error Statistics ===");
    println!("Author ID failures in diff: {}", author_id_fails_diff);
    println!(
        "Author timestamp failures in diff: {}",
        author_ts_fails_diff
    );
    println!("Root directory errors in diff: {}", root_dir_err);
    println!("Path name UTF-8 errors in diff: {}", path_name_err);
}
