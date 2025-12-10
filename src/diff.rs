use crate::env::{FileInfo, RepositoryStats};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex},
};
use swh_graph::{NodeType, graph::SwhFullGraph, labels::EdgeLabel};
use swh_graph_stdlib::diff::{TreeDiffOperation, tree_diff_dirs};

// LRU cache for directory contents (optimization #1)
static DIR_CACHE: once_cell::sync::Lazy<Mutex<lru::LruCache<usize, Arc<HashMap<String, usize>>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(lru::LruCache::new(std::num::NonZeroUsize::new(5000).unwrap())));

pub static ROOT_DIR_ERR: AtomicUsize = AtomicUsize::new(0);
pub static NAME_UTF8_ERR: AtomicUsize = AtomicUsize::new(0);
pub static AUTHOR_ID_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static AUTHOR_TIMESTAMP_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static TREE_DIFF_ERR: AtomicUsize = AtomicUsize::new(0);

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
        format!("author:{:064x}", author)
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

    let (stats, final_added, deleted_files, modified_files, renamed_files) =
        compute_diff_stats(commit_root_dir, &parent_root_dirs, graph);

    // Update FileSystem with the diff results
    update_file_system(
        repo,
        author_id,
        commit_timestamp,
        stats,
        final_added,
        deleted_files,
        modified_files,
        renamed_files,
    );
}

pub fn get_root_dir<G: SwhFullGraph>(commit: usize, graph: &G) -> Option<usize> {
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

pub fn compute_diff_stats<G: SwhFullGraph>(
    commit_root_dir: usize,
    parent_root_dirs: &[usize],
    graph: &G,
) -> (
    DiffStats,
    HashMap<String, usize>,
    HashMap<String, HashSet<usize>>,
    HashMap<String, usize>,
    HashMap<String, (String, usize)>,
    // HashMap<String, HashSet<usize>>,
) {
    // Initialize aggregated diff results with pre-allocation
    let mut all_added: HashMap<String, usize> = HashMap::with_capacity(64);

    // Handle case with no parents (initial commit)
    if parent_root_dirs.is_empty() {
        // For initial commits, all files in the tree are added
        all_added = collect_all_files_in_tree_cached(commit_root_dir, graph);
        return (
            DiffStats {
                added: all_added.len(),
                deleted: 0,
                modified: 0,
                renamed: 0,
            },
            all_added,
            HashMap::with_capacity(0),
            HashMap::with_capacity(0),
            HashMap::with_capacity(0),
            // HashMap::with_capacity(0),
        );
    }

    // Pre-allocate HashMaps
    let mut all_deleted: HashMap<String, HashSet<usize>> = HashMap::with_capacity(32);
    let mut all_modified: HashMap<String, usize> = HashMap::with_capacity(64); // path -> new_file content
    let mut all_renamed: HashMap<String, (String, usize)> = HashMap::with_capacity(16);
    // let mut parent_files: HashMap<String, HashSet<usize>> = HashMap::new();

    // For merge handling: use hybrid approach with enum for zero-cost non-merge case
    #[derive(Default)]
    struct MergeComparison {
        all_added: HashMap<String, usize>,
        all_deleted: HashMap<String, HashSet<usize>>,
        all_modified: HashMap<String, usize>,
    }

    enum MergeTracking {
        None,
        Active(HashMap<usize, MergeComparison>),
    }

    let mut merge_tracking = if parent_root_dirs.len() > 1 {
        MergeTracking::Active(HashMap::new())
    } else {
        MergeTracking::None
    };
    // let mut parent_file_contents: HashMap<String, BTreeSet<usize>> = if is_merge {
    //     HashMap::new()
    // } else {
    //     HashMap::with_capacity(0)
    // };
    // let mut paths_absent_in_parents: HashSet<String> = if is_merge {
    //     HashSet::new()
    // } else {
    //     HashSet::with_capacity(0)
    // };

    // Compute diff against each parent and aggregate results
    for &parent_root_dir in parent_root_dirs {
        match tree_diff_dirs(graph, parent_root_dir, commit_root_dir, None) {
            Ok(diff) => {
                if let MergeTracking::Active(ref mut merge_comparison) = merge_tracking {
                    merge_comparison.insert(parent_root_dir, MergeComparison::default());
                }
                for op in diff.operations() {
                    match op {
                        TreeDiffOperation::Added { path, new_file, .. } => {
                            let path_str = convert_path_to_string(path.as_ref(), graph);
                            // Track as potentially added (will filter later for merges)
                            all_added.entry(path_str.clone()).or_insert(new_file);
                            // This path didn't exist in this parent (only track for merges)
                            if let MergeTracking::Active(ref mut merge_comparison) = merge_tracking
                            {
                                merge_comparison
                                    .entry(parent_root_dir)
                                    .or_default()
                                    .all_added
                                    .entry(path_str.clone())
                                    .or_insert(new_file);
                            }
                        }
                        TreeDiffOperation::Deleted { path, old_file, .. } => {
                            let path_str = convert_path_to_string(path.as_ref(), graph);
                            // Track in all_deleted and parent_files
                            all_deleted
                                .entry(path_str.clone())
                                .or_insert_with(HashSet::new)
                                .insert(old_file);
                            // parent_files
                            //     .entry(path_str.clone())
                            //     .or_insert_with(HashSet::new)
                            //     .insert(old_file);
                            // Track this file's content exists in this parent (only for merges)
                            if let MergeTracking::Active(ref mut merge_comparison) = merge_tracking
                            {
                                merge_comparison
                                    .entry(parent_root_dir)
                                    .or_default()
                                    .all_deleted
                                    .entry(path_str.clone())
                                    .or_insert_with(HashSet::new)
                                    .insert(old_file);
                            }
                        }
                        TreeDiffOperation::Modified {
                            path,
                            old_file: _old_file,
                            new_file,
                            ..
                        } => {
                            let path_str = convert_path_to_string(path.as_ref(), graph);
                            // Track the new content for this path
                            all_modified.entry(path_str.clone()).or_insert(new_file);
                            // Track in parent_files
                            // parent_files
                            //     .entry(path_str.clone())
                            //     .or_insert_with(HashSet::new)
                            //     .insert(old_file);
                            // Track the OLD content in this parent (only for merges)
                            if let MergeTracking::Active(ref mut merge_comparison) = merge_tracking
                            {
                                merge_comparison
                                    .entry(parent_root_dir)
                                    .or_default()
                                    .all_modified
                                    .entry(path_str.clone())
                                    .or_insert(new_file);
                            }
                        }
                        TreeDiffOperation::Moved { .. } => {
                            // Moved variant is never returned according to docs
                            // but we need to handle it for exhaustiveness
                        }
                    }
                }
            }
            Err(_e) => {
                // Failed to compute diff for this parent, skip it
                TREE_DIFF_ERR.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        }
    }

    // For merges: filter operations that are just choosing one parent's version
    if let MergeTracking::Active(merge_comparison) = merge_tracking {
        // Keep in all_added only paths that are present in ALL parents' all_added
        all_added.retain(|path, _| {
            merge_comparison
                .values()
                .all(|parent_cmp| parent_cmp.all_added.contains_key(path))
        });

        // Keep in all_deleted only paths that are present in ALL parents' all_deleted
        all_deleted.retain(|path, _| {
            merge_comparison
                .values()
                .all(|parent_cmp| parent_cmp.all_deleted.contains_key(path))
        });

        // Keep in all_modified only paths that are present in ALL parents' all_added, all_deleted or all_modified
        all_modified.retain(|path, _| {
            merge_comparison.values().all(|parent_cmp| {
                parent_cmp.all_added.contains_key(path)
                    || parent_cmp.all_deleted.contains_key(path)
                    || parent_cmp.all_modified.contains_key(path)
            })
        });
    }

    // Detect renames: same content hash (node ID), different path
    let deleted_hashes: HashMap<usize, HashSet<String>> = {
        let mut map = HashMap::new();
        for (path, node_ids) in &all_deleted {
            for node_id in node_ids {
                map.entry(*node_id)
                    .or_insert_with(HashSet::new)
                    .insert(path.clone());
            }
        }
        map
    };

    // Detect renames by checking if added files were deleted elsewhere
    let mut final_added = HashMap::with_capacity(all_added.len());
    for (added_path, added_node_id) in all_added {
        if let Some(deleted_paths) = deleted_hashes.get(&added_node_id) {
            // Same content, different path = rename
            // Remove this specific node_id from the deleted set
            if let Some(path_to_remove) = deleted_paths.iter().next() {
                if let Some(node_set) = all_deleted.get_mut(path_to_remove) {
                    node_set.remove(&added_node_id);
                    // If the set is now empty, remove the path entirely
                    if node_set.is_empty() {
                        all_deleted.remove(path_to_remove);
                    }
                }
                // Track new_path -> (old_path, node_id)
                all_renamed.insert(added_path, (path_to_remove.clone(), added_node_id));
            }
        } else {
            // Not a rename, it's a true addition
            final_added.insert(added_path, added_node_id);
        }
    }

    let stats = DiffStats {
        added: final_added.len(),
        deleted: all_deleted.len(),
        modified: all_modified.len(),
        renamed: all_renamed.len(),
    };

    (
        stats,
        final_added,
        all_deleted,
        all_modified,
        all_renamed,
        // parent_files,
    )
}

/// Convert a path represented as LabelNameId slice to a String path
fn convert_path_to_string<G: SwhFullGraph>(
    path: &[swh_graph::labels::LabelNameId],
    graph: &G,
) -> String {
    let mut result = String::new();
    for (i, label_id) in path.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        match String::from_utf8(graph.properties().label_name(*label_id)) {
            Ok(name) => result.push_str(&name),
            Err(_) => {
                NAME_UTF8_ERR.fetch_add(1, Ordering::Relaxed);
                result.push_str("<invalid-utf8>");
            }
        }
    }
    if result.is_empty() {
        result.push('.');
    }
    result
}

/// Cached version of collect_all_files_in_tree 
fn collect_all_files_in_tree_cached<G: SwhFullGraph>(
    root_dir: usize,
    graph: &G,
) -> HashMap<String, usize> {
    // Check cache first
    if let Ok(mut cache) = DIR_CACHE.lock() {
        if let Some(cached) = cache.get(&root_dir) {
            return (**cached).clone();
        }
    }
    
    let files = collect_all_files_in_tree_optimized(root_dir, graph);
    
    // Store in cache
    if let Ok(mut cache) = DIR_CACHE.lock() {
        cache.put(root_dir, Arc::new(files.clone()));
    }
    
    files
}

/// Optimized version with pre-allocation and string buffer reuse
fn collect_all_files_in_tree_optimized<G: SwhFullGraph>(
    root_dir: usize,
    graph: &G,
) -> HashMap<String, usize> {
    // Pre-allocate with reasonable capacity
    let mut files = HashMap::with_capacity(256);
    let mut path_node = HashMap::with_capacity(64);
    let mut to_visit = Vec::with_capacity(64);
    let mut visited = HashSet::with_capacity(256);
    
    path_node.insert(root_dir, String::from("."));
    to_visit.push(root_dir);

    while let Some(node) = to_visit.pop() {
        if !visited.insert(node) {
            continue; // Already visited
        }

        // Clone the path to avoid borrow checker issues
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
                    let mut p = String::with_capacity(current_path.len() + 1 + name.len());
                    p.push_str(&current_path);
                    p.push('/');
                    p.push_str(&name);
                    p
                };

                match graph.properties().node_type(succ) {
                    NodeType::Content => {
                        files.insert(path, succ);
                    }
                    NodeType::Directory => {
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
    files
}

/// Original implementation (deprecated - kept for reference)
#[allow(dead_code)]
fn collect_all_files_in_tree<G: SwhFullGraph>(
    root_dir: usize,
    graph: &G,
) -> HashMap<String, usize> {
    let mut files = HashMap::new();
    let mut path_node: HashMap<usize, String> = HashMap::new();
    path_node.insert(root_dir, ".".to_string());

    let mut to_visit = vec![root_dir];
    let mut visited = HashSet::new();

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
                        files.insert(path, succ);
                    }
                    NodeType::Directory => {
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
    files
}

/// Helper function to update file info and interactions
fn update_file_interaction(
    repo: &mut RepositoryStats,
    path: String,
    author_id: &str,
    commit_timestamp: i64,
    set_creator: bool,
) {
    let extension = String::from(
        std::path::Path::new(&path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or(""),
    );

    let file_info = repo
        .file_system
        .files
        .entry(path)
        .or_insert_with(|| FileInfo {
            creator: if set_creator {
                author_id.to_string()
            } else {
                String::new()
            },
            interactions: HashMap::new(),
            ownership_score: HashMap::new(),
            commit_time: Vec::new(),
        });

    // If file already exists and creator should be set, update creator if empty
    if set_creator && file_info.creator.is_empty() {
        file_info.creator = author_id.to_string();
    }

    // Update interactions
    *file_info
        .interactions
        .entry(author_id.to_string())
        .or_insert(0) += 1;
    file_info.commit_time.push(commit_timestamp);

    // Update extensions
    repo.contributors
        .get_mut(author_id)
        .unwrap()
        .extensions
        .entry(extension.clone())
        .and_modify(|count| *count += 1)
        .or_insert(1);
    repo.file_type_interaction
        .entry(extension)
        .and_modify(|count| *count += 1)
        .or_insert(1);
}

fn update_file_system(
    repo: &mut RepositoryStats,
    author_id: String,
    commit_timestamp: i64,
    stats: DiffStats,
    final_added: HashMap<String, usize>,
    deleted_files: HashMap<String, HashSet<usize>>,
    modified_files: HashMap<String, usize>,
    renamed_files: HashMap<String, (String, usize)>,
) {
    // Update global counters
    repo.file_system.files_added += stats.added;
    repo.file_system.files_modified += stats.modified;
    repo.file_system.files_removed += stats.deleted;
    repo.file_system.files_renamed += stats.renamed;

    // For added files: set creator and update interactions
    for (path, _node_id) in final_added {
        update_file_interaction(repo, path, &author_id, commit_timestamp, true);
    }

    // For deleted files: update interactions
    for (path, _node_ids) in deleted_files {
        update_file_interaction(repo, path, &author_id, commit_timestamp, false);
    }

    // For modified files: update interactions
    for (path, _node_id) in modified_files {
        update_file_interaction(repo, path, &author_id, commit_timestamp, false);
    }

    // For renamed files: update interactions
    for (path, _node_id) in renamed_files {
        update_file_interaction(repo, path, &author_id, commit_timestamp, false);
    }
}
