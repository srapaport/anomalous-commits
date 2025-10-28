use anyhow::Result;
use num_cpus;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::java_compat::mph::gov::GOVMPH;
use swh_graph::labels::{EdgeLabel, VisitStatus};

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

pub fn parsing_graph_repositories(graph_basename: String) -> Result<()> {
    let graph = SwhBidirectionalGraph::new(graph_basename)?
        .load_all_properties::<GOVMPH>()?
        .load_forward_labels()?
        .load_backward_labels()?;
    let props = graph.properties();

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
                // We retrieve the latest snapshot of the origin
                let Some(latest_snapshot) = retrieve_latest_snapshots(node, &graph) else {
                    continue;
                };

                // We process the latest snapshot in a thread
                let graph = &graph;
                s.spawn(move |_| {
                    process_snapshot(latest_snapshot, graph);
                });
            }
        });
    });

    Ok(())
}

/// #Files added
/// #Files removed
/// #Files modified
/// #Files renamed
/// #Unique file types modified per repo
/// Proportion of ownership
/// Number of commits for an author in a repo
/// Time since 1st commit of an author in a repo
/// Commit-time distribution per repo
/// Time between commits
/// Time between commits to a file
/// File changed by many dev
/// Introduction of prebuilt binaries
/// Type of files changed per dev Xrepo
fn process_snapshot<G: SwhFullGraph>(snapshot: usize, graph: &G) {
    
}
