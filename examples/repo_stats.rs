use anyhow::Result;
use rocksdb::{DB, Options};
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new(
        "/home/infres/rapaport/datasets/2025-05-28-popular-1k/compressed/graph",
    )?
    .load_all_properties::<DynMphf>()?
    .load_forward_labels()?
    .load_backward_labels()?;

    let db_path = "/home/infres/rapaport/datasets/2025-05-28-popular-1k/results/RepositoryStats.db";

    if std::path::Path::new(db_path).exists() {
        DB::destroy(&Options::default(), db_path)?;
    }

    for node in 0..graph.num_nodes() {
        if graph.properties().node_type(node) != NodeType::Origin {
            continue;
        }
        let Some(url) = graph.properties().message(node) else {
            continue;
        };
        let Ok(url_string) = String::from_utf8(url) else {
            continue;
        };
        let Some(latest_snapshot) =
            anomalous_commits::outliers::retrieve_latest_snapshots(node, &graph)
        else {
            continue;
        };
        let mut repo = anomalous_commits::outliers::process_snapshot(latest_snapshot, &graph);
        repo.flush_to_rocksdb(
            url_string,
            db_path,
        )
        .unwrap();
        break;
    }
    Ok(())
}
