use crate::env::{FileInfo, RepositoryStats};
use std::{
    collections::{BTreeSet, HashMap},
    sync::atomic::{AtomicUsize, Ordering},
};
use swh_graph::{NodeType, graph::SwhFullGraph, labels::EdgeLabel};

pub static ROOT_DIR_ERR: AtomicUsize = AtomicUsize::new(0);
pub static NAME_UTF8_ERR: AtomicUsize = AtomicUsize::new(0);
pub static AUTHOR_ID_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static AUTHOR_TIMESTAMP_FAILURES: AtomicUsize = AtomicUsize::new(0);

pub fn compute_diff<G: SwhFullGraph>(
    commit: usize,
    parents: Vec<usize>,
    repo: &mut RepositoryStats,
    graph: &G,
) {
    let Some(commit_root_dir) = get_root_dir(commit, graph) else {
        ROOT_DIR_ERR.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let parent_root_dirs: Vec<usize> = parents
        .into_iter()
        .filter_map(|parent| get_root_dir(parent, graph))
        .collect();

    // Get author and timestamp for this commit
    let props = graph.properties();
    let author_id = if let Some(author) = props.author_id(commit) {
        format!("author:{:x}", author)
    } else {
        AUTHOR_ID_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let commit_timestamp = if let Some(ts) = props.author_timestamp(commit) {
        ts
    } else {
        AUTHOR_TIMESTAMP_FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let (stats, final_added, deleted_files, modified_files, parent_files) =
        compute_diff_stats(commit_root_dir, &parent_root_dirs, graph);

    // Initialize files from parent trees if not already present
    for (path, _) in parent_files {
        repo.file_system
            .files
            .entry(path.clone())
            .or_insert_with(|| {
                FileInfo {
                    creator: String::new(), // Will be set when file is added
                    interactions: HashMap::new(),
                    ownership_score: HashMap::new(),
                    commit_time: Vec::new(),
                }
            });
    }

    // Update FileSystem with the diff results
    update_file_system(
        repo,
        author_id,
        commit_timestamp,
        stats,
        final_added,
        deleted_files,
        modified_files,
    );
}

fn get_root_dir<G: SwhFullGraph>(commit: usize, graph: &G) -> Option<usize> {
    for succ in graph.successors(commit) {
        if graph.properties().node_type(succ) == NodeType::Directory {
            return Some(succ);
        }
    }
    return None;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffStats {
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
    pub renamed: usize,
}

fn compute_diff_stats<G: SwhFullGraph>(
    commit_root_dir: usize,
    parent_root_dirs: &[usize],
    graph: &G,
) -> (
    DiffStats,
    HashMap<String, usize>,
    HashMap<String, BTreeSet<usize>>,
    Vec<String>,
    HashMap<String, BTreeSet<usize>>,
) {
    // Build union of all parent trees: path -> set of node IDs (content hashes)
    // Also track parent directories for optimization
    let (parent_files, parent_dirs) = get_commit_trees(parent_root_dirs, graph);

    // Track changes while traversing commit tree
    let mut added_files: HashMap<String, usize> = HashMap::new();
    let mut modified_files: Vec<String> = Vec::new();
    let mut deleted_files = parent_files.clone();

    // Traverse commit tree and compare on-the-fly
    traverse_and_compare(
        commit_root_dir,
        graph,
        &parent_files,
        &parent_dirs,
        &mut deleted_files,
        &mut added_files,
        &mut modified_files,
    );

    // Detect renames: same content hash (node ID), different path
    let mut renamed_count = 0;
    let deleted_hashes: HashMap<usize, BTreeSet<String>> = {
        let mut map = HashMap::new();
        for (path, node_ids) in &deleted_files {
            for node_id in node_ids {
                map.entry(*node_id)
                    .or_insert_with(BTreeSet::new)
                    .insert(path.clone());
            }
        }
        map
    };

    let mut final_added = added_files.clone();
    for (added_path, added_node_id) in added_files.into_iter() {
        if let Some(deleted_paths) = deleted_hashes.get(&added_node_id) {
            // Same content, different path = rename
            renamed_count += 1;
            final_added.remove(&added_path);
            // Remove one instance of this hash from deleted
            if let Some(path_to_remove) = deleted_paths.iter().next() {
                deleted_files.remove(path_to_remove);
            }
        }
    }

    let stats = DiffStats {
        added: final_added.len(),
        deleted: deleted_files.len(),
        modified: modified_files.len(),
        renamed: renamed_count,
    };

    (
        stats,
        final_added,
        deleted_files,
        modified_files,
        parent_files,
    )
}

fn update_file_system(
    repo: &mut RepositoryStats,
    author_id: String,
    commit_timestamp: i64,
    stats: DiffStats,
    final_added: HashMap<String, usize>,
    deleted_files: HashMap<String, BTreeSet<usize>>,
    modified_files: Vec<String>,
) {
    // Update global counters
    repo.file_system.files_added += stats.added;
    repo.file_system.files_modified += stats.modified;
    repo.file_system.files_removed += stats.deleted;
    repo.file_system.files_renamed += stats.renamed;

    // For added files: set creator and update interactions
    for (path, _node_id) in final_added {
        let file_info = repo
            .file_system
            .files
            .entry(path)
            .or_insert_with(|| FileInfo {
                creator: author_id.to_string(),
                interactions: HashMap::new(),
                ownership_score: HashMap::new(),
                commit_time: Vec::new(),
            });

        // If file already exists (shouldn't happen for added files), update creator if empty
        if file_info.creator.is_empty() {
            file_info.creator = author_id.to_string();
        }

        // Update interactions
        *file_info
            .interactions
            .entry(author_id.to_string())
            .or_insert(0) += 1;
        file_info.commit_time.push(commit_timestamp);
    }

    // For deleted files: update interactions
    for (path, _node_ids) in deleted_files {
        if let Some(file_info) = repo.file_system.files.get_mut(&path) {
            *file_info
                .interactions
                .entry(author_id.to_string())
                .or_insert(0) += 1;
            file_info.commit_time.push(commit_timestamp);
        }
    }

    // For modified files: update interactions
    for path in modified_files {
        if let Some(file_info) = repo.file_system.files.get_mut(&path) {
            *file_info
                .interactions
                .entry(author_id.to_string())
                .or_insert(0) += 1;
            file_info.commit_time.push(commit_timestamp);
        }
    }
}

fn traverse_and_compare<G: SwhFullGraph>(
    root_dir: usize,
    graph: &G,
    parent_files: &HashMap<String, BTreeSet<usize>>,
    parent_dirs: &HashMap<String, BTreeSet<usize>>,
    deleted_files: &mut HashMap<String, BTreeSet<usize>>,
    added_files: &mut HashMap<String, usize>,
    modified_files: &mut Vec<String>,
) {
    let mut path_node: HashMap<usize, String> = HashMap::new();
    path_node.insert(root_dir, ".".to_string());

    let mut to_visit = vec![root_dir];
    let mut visited = BTreeSet::new();

    while let Some(node) = to_visit.pop() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node);

        let current_path = path_node
            .get(&node)
            .expect("couldn't find path in path_node")
            .clone();

        for (succ, labels) in graph.labeled_successors(node) {
            for label in labels {
                let name = if let EdgeLabel::DirEntry(dir_entry) = label {
                    if let Ok(name) =
                        String::from_utf8(graph.properties().label_name(dir_entry.label_name_id()))
                    {
                        name
                    } else {
                        NAME_UTF8_ERR.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                } else {
                    continue;
                };

                let path = if current_path == "." {
                    name
                } else {
                    format!("{}/{}", current_path, name)
                };

                match graph.properties().node_type(succ) {
                    NodeType::Content => {
                        // Node ID is the content hash
                        let node_id = succ;

                        // Compare with parent union on-the-fly
                        if let Some(parent_node_ids) = parent_files.get(&path) {
                            // File exists in parent(s)
                            if !parent_node_ids.contains(&node_id) {
                                // Node ID changed = modified
                                modified_files.push(path.clone());
                            }
                            // Remove from deleted (it exists in commit)
                            deleted_files.remove(&path);
                        } else {
                            // File doesn't exist in any parent = added (or renamed)
                            added_files.insert(path, node_id);
                        }
                    }
                    NodeType::Directory => {
                        // Check if this directory exists in parent with same node ID
                        if let Some(parent_dir_ids) = parent_dirs.get(&path) {
                            if parent_dir_ids.contains(&succ) {
                                // Directory unchanged - skip entire subtree!
                                // Remove all files under this path from deleted_files
                                let prefix = if path == "." {
                                    String::new()
                                } else {
                                    format!("{}/", path)
                                };
                                deleted_files.retain(|file_path, _| {
                                    !file_path.starts_with(&prefix) && file_path != &path
                                });
                                continue; // Skip adding to to_visit
                            }
                        }

                        // Directory changed or new - traverse it
                        path_node.insert(succ, path);
                        if !visited.contains(&succ) {
                            to_visit.push(succ);
                        }
                    }
                    _ => continue,
                }
            }
        }
    }
}

fn get_commit_trees<G: SwhFullGraph>(
    root_dirs: &[usize],
    graph: &G,
) -> (
    HashMap<String, BTreeSet<usize>>,
    HashMap<String, BTreeSet<usize>>,
) {
    let mut files: HashMap<String, BTreeSet<usize>> = HashMap::new();
    let mut dirs: HashMap<String, BTreeSet<usize>> = HashMap::new();

    for &root_dir in root_dirs {
        let mut path_node: HashMap<usize, String> = HashMap::new();
        path_node.insert(root_dir, ".".to_string());

        let mut to_visit = vec![root_dir];
        let mut visited = BTreeSet::new();

        while let Some(node) = to_visit.pop() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);
            let current_path = path_node
                .get(&node)
                .expect("couldn't find path in path_node")
                .clone();
            for (succ, labels) in graph.labeled_successors(node) {
                for label in labels {
                    let name = if let EdgeLabel::DirEntry(dir_entry) = label {
                        if let Ok(name) = String::from_utf8(
                            graph.properties().label_name(dir_entry.label_name_id()),
                        ) {
                            name
                        } else {
                            NAME_UTF8_ERR.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    } else {
                        continue;
                    };
                    let path = if current_path == "." {
                        name
                    } else {
                        format!("{}/{}", current_path, name)
                    };
                    match graph.properties().node_type(succ) {
                        NodeType::Content => {
                            // Store path -> node ID (content hash)
                            files.entry(path).or_insert_with(BTreeSet::new).insert(succ);
                        }
                        NodeType::Directory => {
                            // Store directory path -> node ID
                            dirs.entry(path.clone())
                                .or_insert_with(BTreeSet::new)
                                .insert(succ);
                            path_node.insert(succ, path);
                            if !visited.contains(&succ) {
                                to_visit.push(succ);
                            }
                        }
                        _ => continue,
                    }
                }
            }
        }
    }
    (files, dirs)
}
