use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::Result;
use swh_graph::NodeType;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn main() -> Result<()> {
    let graph = SwhBidirectionalGraph::new("/swh/scratch/graph/2025-05-18/compressed/graph")?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let root_commit = "swh:1:rev:847047cf7f81ab08352038b2204f0e7633449580";
    let swhid = swh_graph::SWHID::from_str(root_commit)?;
    let root_commit_id = graph.properties().node_id(swhid)?;
    let mut origins = vec![];

    let mut to_visit = vec![root_commit_id];
    let mut visited = BTreeSet::new();
    while let Some(node) = to_visit.pop() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node);

        for pred in graph.predecessors(node) {
            match graph.properties().node_type(pred) {
                NodeType::Revision | NodeType::Release | NodeType::Snapshot => {
                    to_visit.push(pred);
                }
                NodeType::Origin => {
                    origins.push({
                        let bytes = graph.properties().message(pred).unwrap();
                        String::from_utf8(bytes).unwrap()
                    });
                }
                _ => (),
            }
        }
    }
    origins
        .into_iter()
        .for_each(|ori| println!("origin containing sus: {}", ori));
    Ok(())
}
