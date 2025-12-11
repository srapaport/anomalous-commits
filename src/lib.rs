pub mod diff;
pub mod env;
pub mod outliers;
pub mod score;
pub mod time_of_day;

use anyhow::Result;
use chashmap::CHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use orc_rust::ArrowReaderBuilder;
use rayon::prelude::*;
use rocksdb::{DB, Options};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use swh_graph::NodeType;
use swh_graph::graph::SwhFullGraph;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

use crate::env::{AuthorInfo, CommitterInfo};

static URL_ERRORS: AtomicUsize = AtomicUsize::new(0);
static URL_ERRORS_UTF8: AtomicUsize = AtomicUsize::new(0);
static EMPTY_REPO: AtomicUsize = AtomicUsize::new(0);

pub fn flush_to_rocksdb(
    authors: env::Authors,
    committers: env::Committers,
    db_path: &str,
) -> Result<(), rocksdb::Error> {
    // Delete the database if it exists to start fresh
    if std::path::Path::new(db_path).exists() {
        DB::destroy(&Options::default(), db_path)?;
    }

    // Open/create RocksDB database
    let mut opts = Options::default();
    opts.create_if_missing(true);
    // Set max value size to 1GB
    opts.set_write_buffer_size(1024 * 1024 * 1024);
    let db = DB::open(&opts, db_path)?;

    // max size for teaser 328_447_742
    const MAX_RECORD_SIZE: usize = 500 * 1024 * 1024; // 500MB in bytes
    let mut max_size = (String::new(), 0);
    let mut skipped_authors = 0;
    let mut truncated_authors = 0;

    for (author_id, author_info) in authors.into_iter() {
        let mut person_record = env::PersonRecord {
            person_type: env::PersonType::Author,
            name: author_info.nom,
            email: author_info.email,
            number_of_commits: author_info.number_of_commits,
            number_of_self_commits: author_info.number_of_self_commits,
            time_since_first_commit: author_info.time_since_first_commit,
            commits: author_info.commits,
            extensions: HashMap::new(),
        };
        person_record
            .commits
            .sort_unstable_by_key(|commit| commit.1);
        let mut value =
            serde_json::to_vec(&person_record).expect("Failed to serialize author info");
        let length = value.len();
        if length > max_size.1 {
            max_size.0 = author_id.clone();
            max_size.1 = length;
        }
        // Check if the value is too large
        if length > MAX_RECORD_SIZE {
            log::warn!(
                "Author {} has {} commits, serialized size: {:.2} MB - truncating commits",
                author_id,
                person_record.commits.len(),
                length as f64 / (1024.0 * 1024.0)
            );

            // Truncate commits to first and last 1000
            if person_record.commits.len() > 2000 {
                let mut truncated_commits = Vec::with_capacity(2000);
                truncated_commits.extend_from_slice(&person_record.commits[..1000]);
                truncated_commits.extend_from_slice(
                    &person_record.commits[person_record.commits.len() - 2000..],
                );
                person_record.commits = truncated_commits;
            } else {
                skipped_authors += 1;
                continue;
            }

            value = serde_json::to_vec(&person_record)
                .expect("Failed to serialize truncated author info");

            // If STILL too large after truncation, skip this author
            if value.len() > MAX_RECORD_SIZE {
                log::error!(
                    "Author {} still too large after truncation ({:.2} MB) - skipping",
                    author_id,
                    value.len() as f64 / (1024.0 * 1024.0)
                );
                skipped_authors += 1;
                continue;
            }
            truncated_authors += 1;
        }

        // Store in RocksDB with "author:<author_id>" as key
        let key = format!("author:{}", author_id);
        db.put(key.as_bytes(), value)?;
    }

    let mut skipped_committers = 0;
    let mut truncated_committers = 0;

    for (committer_id, committer_info) in committers.into_iter() {
        let mut person_record = env::PersonRecord {
            person_type: env::PersonType::Committer,
            name: committer_info.nom,
            email: committer_info.email,
            number_of_commits: committer_info.number_of_commits,
            number_of_self_commits: committer_info.number_of_self_commits,
            time_since_first_commit: committer_info.time_since_first_commit,
            commits: committer_info.commits,
            extensions: HashMap::new(),
        };
        person_record
            .commits
            .sort_unstable_by_key(|commit| commit.1);
        let mut value =
            serde_json::to_vec(&person_record).expect("Failed to serialize committer info");
        let length = value.len();
        if length > max_size.1 {
            max_size.0 = committer_id.clone();
            max_size.1 = length;
        }
        // Check if the value is too large
        if value.len() > MAX_RECORD_SIZE {
            log::warn!(
                "Committer {} has {} commits, serialized size: {:.2} MB - truncating commits",
                committer_id,
                person_record.commits.len(),
                value.len() as f64 / (1024.0 * 1024.0)
            );

            // Truncate commits to first and last 1000
            if person_record.commits.len() > 2000 {
                let mut truncated_commits = Vec::with_capacity(2000);
                truncated_commits.extend_from_slice(&person_record.commits[..1000]);
                truncated_commits.extend_from_slice(
                    &person_record.commits[person_record.commits.len() - 1000..],
                );
                person_record.commits = truncated_commits;
            } else {
                skipped_committers += 1;
                continue;
            }

            value = serde_json::to_vec(&person_record)
                .expect("Failed to serialize truncated committer info");

            // If STILL too large after truncation, skip this committer
            if value.len() > MAX_RECORD_SIZE {
                log::error!(
                    "Committer {} still too large after truncation ({:.2} MB) - skipping",
                    committer_id,
                    value.len() as f64 / (1024.0 * 1024.0)
                );
                skipped_committers += 1;
                continue;
            }
            truncated_committers += 1;
        }

        // Store in RocksDB with "committer:<committer_id>" as key
        let key = format!("committer:{}", committer_id);
        db.put(key.as_bytes(), value)?;
    }

    db.flush()?;

    // Print summary
    if truncated_authors > 0
        || skipped_authors > 0
        || truncated_committers > 0
        || skipped_committers > 0
    {
        println!("\n========== Flush Summary ==========");
        println!("  - Authors truncated:     {}", truncated_authors);
        println!("  - Authors skipped:       {}", skipped_authors);
        println!("  - Committers truncated:  {}", truncated_committers);
        println!("  - Committers skipped:    {}", skipped_committers);
        println!("===================================\n");
    }
    println!("max size saved: {} for {}", max_size.1, max_size.0);
    Ok(())
}

pub fn extract_author_commits(dir_orc: String) {
    let files = std::fs::read_dir(dir_orc)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    // Error counters for column-level failures
    let error_commit_ids = AtomicUsize::new(0);
    let error_author_ids = AtomicUsize::new(0);
    let error_author_dates = AtomicUsize::new(0);
    let error_author_timezones = AtomicUsize::new(0);
    let error_committer_ids = AtomicUsize::new(0);
    let error_committer_dates = AtomicUsize::new(0);
    let error_committer_timezones = AtomicUsize::new(0);

    // Error counters for row-level missing data
    let error_missing_commit_id = AtomicUsize::new(0);
    let error_missing_author_id = AtomicUsize::new(0);
    let error_missing_author_date = AtomicUsize::new(0);
    let error_missing_author_tz = AtomicUsize::new(0);
    let error_missing_committer_id = AtomicUsize::new(0);
    let error_missing_committer_date = AtomicUsize::new(0);
    let error_missing_committer_tz = AtomicUsize::new(0);

    let all_authors = env::Authors::new();
    let all_committers = env::Committers::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let bar = ProgressBar::new(files.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} {wide_bar} {pos} {percent_precise}% {elapsed_precise} {eta}",
        )
        .unwrap(),
    );
    bar.set_message("Retrieving Author Infos");
    files.into_par_iter().for_each(|dir_entry| {
        let orc_path = dir_entry.path();
        info!(
            "orc_path : {}",
            orc_path
                .file_name()
                .unwrap()
                .to_os_string()
                .into_string()
                .unwrap()
        );
        let file = File::open(dir_entry.path())
            .expect(format!("could not open .orc {:?}", orc_path).as_str());
        let reader = ArrowReaderBuilder::try_new(file).unwrap().build();
        let record_batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        for record in record_batches {
            // Commit id
            let Some(ids) = record
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .map(|col| col.iter().collect::<Vec<Option<&str>>>())
            else {
                error_commit_ids.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author id
            let Some(authors) = record
                .column(2)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .map(|col| col.iter().collect::<Vec<Option<&[u8]>>>())
            else {
                error_author_ids.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author Date
            let Some(dates) = record
                .column(3)
                .as_any()
                .downcast_ref::<arrow::array::TimestampNanosecondArray>()
                .map(|col| col.iter().collect::<Vec<Option<i64>>>())
            else {
                error_author_dates.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author Timezone
            let Some(tzs) = record
                .column(4)
                .as_any()
                .downcast_ref::<arrow::array::Int16Array>()
                .map(|col| col.iter().collect::<Vec<Option<i16>>>())
            else {
                error_author_timezones.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer id
            let Some(committers) = record
                .column(6)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .map(|col| col.iter().collect::<Vec<Option<&[u8]>>>())
            else {
                error_committer_ids.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer date
            let Some(cdates) = record
                .column(7)
                .as_any()
                .downcast_ref::<arrow::array::TimestampNanosecondArray>()
                .map(|col| col.iter().collect::<Vec<Option<i64>>>())
            else {
                error_committer_dates.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer Timezone
            let Some(ctzs) = record
                .column(8)
                .as_any()
                .downcast_ref::<arrow::array::Int16Array>()
                .map(|col| col.iter().collect::<Vec<Option<i16>>>())
            else {
                error_committer_timezones.fetch_add(1, Ordering::Relaxed);
                continue;
            };

            for row in 0..record.column(0).len() {
                let Some(id) = ids.get(row).and_then(|elem| *elem) else {
                    error_missing_commit_id.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(author) = authors.get(row).and_then(|elem| *elem).map(|bytes| {
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                }) else {
                    error_missing_author_id.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(date) = dates.get(row).and_then(|elem| *elem) else {
                    error_missing_author_date.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(tz) = tzs.get(row).and_then(|elem| *elem) else {
                    error_missing_author_tz.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(committer) = committers.get(row).and_then(|elem| *elem).map(|bytes| {
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                }) else {
                    error_missing_committer_id.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(cdate) = cdates.get(row).and_then(|elem| *elem) else {
                    error_missing_committer_date.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(ctz) = ctzs.get(row).and_then(|elem| *elem) else {
                    error_missing_committer_tz.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let self_committed = committer == author;

                all_authors.upsert(
                    author.clone(),
                    || env::AuthorInfo {
                        nom: None,
                        email: None,
                        number_of_commits: 1,
                        number_of_self_commits: if self_committed { 1 } else { 0 },
                        time_since_first_commit: (now - date),
                        commits: vec![(id.to_owned(), date, tz)],
                    },
                    |author_info| {
                        author_info.time_since_first_commit =
                            author_info.time_since_first_commit.max(now - date);
                        author_info.number_of_commits = author_info.number_of_commits + 1;
                        if self_committed {
                            author_info.number_of_self_commits =
                                author_info.number_of_self_commits + 1;
                        }
                        author_info.commits.push((id.to_owned(), date, tz));
                    },
                );

                all_committers.upsert(
                    committer,
                    || env::CommitterInfo {
                        nom: None,
                        email: None,
                        number_of_commits: 1,
                        number_of_self_commits: if self_committed { 1 } else { 0 },
                        time_since_first_commit: (now - cdate),
                        commits: vec![(id.to_owned(), cdate, ctz)],
                    },
                    |committer_info| {
                        committer_info.time_since_first_commit =
                            committer_info.time_since_first_commit.max(now - cdate);
                        committer_info.number_of_commits = committer_info.number_of_commits + 1;
                        if self_committed {
                            committer_info.number_of_self_commits =
                                committer_info.number_of_self_commits + 1;
                        }
                        committer_info.commits.push((id.to_owned(), cdate, ctz));
                    },
                );
            }
        }
        bar.inc(1);
    });
    bar.finish();

    // Display error statistics
    println!("\n========== Error Report ==========");
    println!("Column-level errors (entire column failed to read):");
    println!(
        "  - Commit IDs:           {}",
        error_commit_ids.load(Ordering::Relaxed)
    );
    println!(
        "  - Author IDs:           {}",
        error_author_ids.load(Ordering::Relaxed)
    );
    println!(
        "  - Author Dates:         {}",
        error_author_dates.load(Ordering::Relaxed)
    );
    println!(
        "  - Author Timezones:     {}",
        error_author_timezones.load(Ordering::Relaxed)
    );
    println!(
        "  - Committer IDs:        {}",
        error_committer_ids.load(Ordering::Relaxed)
    );
    println!(
        "  - Committer Dates:      {}",
        error_committer_dates.load(Ordering::Relaxed)
    );
    println!(
        "  - Committer Timezones:  {}",
        error_committer_timezones.load(Ordering::Relaxed)
    );

    let total_column_errors = error_commit_ids.load(Ordering::Relaxed)
        + error_author_ids.load(Ordering::Relaxed)
        + error_author_dates.load(Ordering::Relaxed)
        + error_author_timezones.load(Ordering::Relaxed)
        + error_committer_ids.load(Ordering::Relaxed)
        + error_committer_dates.load(Ordering::Relaxed)
        + error_committer_timezones.load(Ordering::Relaxed);

    println!("\nRow-level errors (missing data in specific rows):");
    println!(
        "  - Missing Commit ID:       {}",
        error_missing_commit_id.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Author ID:       {}",
        error_missing_author_id.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Author Date:     {}",
        error_missing_author_date.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Author TZ:       {}",
        error_missing_author_tz.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer ID:    {}",
        error_missing_committer_id.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer Date:  {}",
        error_missing_committer_date.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer TZ:    {}",
        error_missing_committer_tz.load(Ordering::Relaxed)
    );

    let total_row_errors = error_missing_commit_id.load(Ordering::Relaxed)
        + error_missing_author_id.load(Ordering::Relaxed)
        + error_missing_author_date.load(Ordering::Relaxed)
        + error_missing_author_tz.load(Ordering::Relaxed)
        + error_missing_committer_id.load(Ordering::Relaxed)
        + error_missing_committer_date.load(Ordering::Relaxed)
        + error_missing_committer_tz.load(Ordering::Relaxed);

    println!("\nTotal Errors:");
    println!("  - Column-level: {}", total_column_errors);
    println!("  - Row-level:    {}", total_row_errors);
    println!(
        "  - Grand Total:  {}",
        total_column_errors + total_row_errors
    );
    println!("==================================\n");

    // Transfer ownership to flush_to_rocksdb - data will be consumed and freed
    flush_to_rocksdb(all_authors, all_committers, "data/persons.db").unwrap();
}

pub fn build_repo_stats<G: SwhFullGraph + Sync>(
    graph: &G,
    db_path: &str,
    list_repo: Option<&[&str]>,
) -> Result<()> {
    if std::path::Path::new(db_path).exists() {
        DB::destroy(&Options::default(), db_path)?;
    }

    // Open DB once and share it across threads
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.set_write_buffer_size(2 * 1024 * 1024 * 1024);
    let db = Arc::new(Mutex::new(DB::open(&opts, db_path)?));

    // max for teaser 975_761_765
    const MAX_RECORD_SIZE: usize = 1024 * 1024 * 1024; // 1GB in bytes
    let skipped_repos = AtomicUsize::new(0);
    let truncated_repos = AtomicUsize::new(0);

    // CSV file to track skipped/truncated repos
    let csv_log = Arc::new(Mutex::new(
        File::create("data/repo_processing_log.csv").expect("Failed to create CSV log file"),
    ));
    // Write CSV header
    writeln!(csv_log.lock().unwrap(), "url,skipped,truncated").expect("Failed to write CSV header");

    let amount_origins = *graph.num_nodes_by_type()?.get(&NodeType::Origin).unwrap();
    let bar = ProgressBar::new(amount_origins as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} {wide_bar} {pos} {percent_precise}% {elapsed_precise} {eta}",
        )
        .unwrap(),
    );
    bar.set_message("Retrieving Repositories Stats");

    let max_size = (Arc::new(Mutex::new(String::new())), AtomicUsize::new(0));

    (0..graph.num_nodes())
        .into_par_iter()
        .filter(|&node| graph.properties().node_type(node) == NodeType::Origin)
        .filter_map(|node| {
            let url = graph.properties().message(node).or_else(|| {
                URL_ERRORS.fetch_add(1, Ordering::Relaxed);
                None
            })?;
            let url_string = String::from_utf8(url).ok().or_else(|| {
                URL_ERRORS_UTF8.fetch_add(1, Ordering::Relaxed);
                None
            })?;
            let latest_snapshot = outliers::retrieve_latest_snapshot(node, graph)?;
            match list_repo {
                Some(repos) => {
                    if repos.contains(&url_string.as_str()) {
                        Some((url_string, latest_snapshot))
                    } else {
                        None
                    }
                }
                None => Some((url_string, latest_snapshot)),
            }
        })
        .for_each(|(url_string, latest_snapshot)| {
            bar.inc(1);
            let start = std::time::Instant::now();
            let mut repo = outliers::process_snapshot(latest_snapshot, graph);
            outliers::compute_ownership_score(&mut repo).unwrap();
            info!(
                "origin: {} | with: {} commits | took {:?}",
                &url_string,
                repo.commit_time.len(),
                start.elapsed()
            );
            if !repo.commit_time.is_empty() {
                // Defensive programming: check serialized size before writing
                match repo.flush_to_rocksdb_async_safe_defensive(
                    url_string.clone(),
                    &db,
                    MAX_RECORD_SIZE,
                    &max_size,
                ) {
                    Ok((was_skipped, was_truncated)) => {
                        if was_skipped {
                            skipped_repos.fetch_add(1, Ordering::Relaxed);
                        }
                        if was_truncated {
                            truncated_repos.fetch_add(1, Ordering::Relaxed);
                        }
                        // Log to CSV
                        writeln!(
                            csv_log.lock().unwrap(),
                            "{},{},{}",
                            url_string,
                            was_skipped,
                            was_truncated
                        )
                        .ok();
                    }
                    Err(e) => {
                        log::error!("Failed to write repo {} to RocksDB: {}", url_string, e);
                        skipped_repos.fetch_add(1, Ordering::Relaxed);
                        writeln!(csv_log.lock().unwrap(), "{},true,false", url_string).ok();
                    }
                }
            } else {
                EMPTY_REPO.fetch_add(1, Ordering::Relaxed);
            }
        });
    print_error_stats();
    outliers::print_error_stats();

    // Print repository flush summary
    let truncated = truncated_repos.load(Ordering::Relaxed);
    let skipped = skipped_repos.load(Ordering::Relaxed);
    if truncated > 0 || skipped > 0 {
        println!("\n========== Repository Flush Summary ==========");
        println!("  - Repositories truncated: {}", truncated);
        println!("  - Repositories skipped:   {}", skipped);
        println!("==============================================\n");
    }

    // Explicitly flush at the end
    println!(
        "Max size of repo without errors: {} for {}",
        max_size.1.load(Ordering::Relaxed),
        *max_size.0.lock().unwrap()
    );
    db.lock().unwrap().flush()?;
    Ok(())
}

fn print_error_stats() {
    // Print error statistics
    println!("\n========== URL Error Report ==========");
    println!(
        "  - Missing URLs:        {}",
        URL_ERRORS.load(Ordering::Relaxed)
    );
    println!(
        "  - Invalid UTF-8 URLs:  {}",
        URL_ERRORS_UTF8.load(Ordering::Relaxed)
    );
    println!("  - No Commits:  {}", EMPTY_REPO.load(Ordering::Relaxed));
    println!(
        "  - Total URL errors:    {}",
        URL_ERRORS.load(Ordering::Relaxed)
            + URL_ERRORS_UTF8.load(Ordering::Relaxed)
            + EMPTY_REPO.load(Ordering::Relaxed)
    );
    println!("======================================\n");
}

pub fn extract_author_commits_graph(graph: &str, data_path: &str) -> Result<()> {
    let all_authors: CHashMap<String, AuthorInfo> = CHashMap::new();
    let all_committers: CHashMap<String, CommitterInfo> = CHashMap::new();

    let graph = SwhBidirectionalGraph::new(graph)?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;
    let props = graph.properties();

    let bar =
        ProgressBar::new(*graph.num_nodes_by_type()?.get(&NodeType::Revision).unwrap() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} {wide_bar} {pos} {percent_precise}% {elapsed_precise} {eta}",
        )
        .unwrap(),
    );
    bar.set_message("Retrieving Author Infos");

    // Error counters
    let error_missing_author_id = AtomicUsize::new(0);
    let error_missing_author_date = AtomicUsize::new(0);
    let error_missing_author_tz = AtomicUsize::new(0);
    let error_missing_committer_id = AtomicUsize::new(0);
    let error_missing_committer_date = AtomicUsize::new(0);
    let error_missing_committer_tz = AtomicUsize::new(0);

    (0..graph.num_nodes())
        .into_par_iter()
        .filter(|node| props.node_type(*node) == NodeType::Revision)
        .for_each(|rev| {
            let id: String = props
                .swhid(rev)
                .hash
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            let author = if let Some(author_id) = props.author_id(rev) {
                format!("{:064x}", author_id)
            } else {
                error_missing_author_id.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Some(date) = props.author_timestamp(rev) else {
                error_missing_author_date.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Some(tz) = props.author_timestamp_offset(rev) else {
                error_missing_author_tz.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let committer = if let Some(committer_id) = props.committer_id(rev) {
                format!("{:064x}", committer_id)
            } else {
                error_missing_committer_id.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Some(cdate) = props.committer_timestamp(rev) else {
                error_missing_committer_date.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Some(ctz) = props.committer_timestamp_offset(rev) else {
                error_missing_committer_tz.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let self_committed = committer == author;
            all_authors.upsert(
                author.clone(),
                || env::AuthorInfo {
                    nom: None,
                    email: None,
                    number_of_commits: 1,
                    number_of_self_commits: if self_committed { 1 } else { 0 },
                    time_since_first_commit: (env::get_now_timestamp() - date),
                    commits: vec![(id.to_owned(), date, tz)],
                },
                |author_info| {
                    author_info.time_since_first_commit = author_info
                        .time_since_first_commit
                        .max(env::get_now_timestamp() - date);
                    author_info.number_of_commits = author_info.number_of_commits + 1;
                    if self_committed {
                        author_info.number_of_self_commits = author_info.number_of_self_commits + 1;
                    }
                    author_info.commits.push((id.to_owned(), date, tz));
                },
            );
            all_committers.upsert(
                committer,
                || env::CommitterInfo {
                    nom: None,
                    email: None,
                    number_of_commits: 1,
                    number_of_self_commits: if self_committed { 1 } else { 0 },
                    time_since_first_commit: (env::get_now_timestamp() - cdate),
                    commits: vec![(id.to_owned(), cdate, ctz)],
                },
                |committer_info| {
                    committer_info.time_since_first_commit = committer_info
                        .time_since_first_commit
                        .max(env::get_now_timestamp() - cdate);
                    committer_info.number_of_commits = committer_info.number_of_commits + 1;
                    if self_committed {
                        committer_info.number_of_self_commits =
                            committer_info.number_of_self_commits + 1;
                    }
                    committer_info.commits.push((id.to_owned(), cdate, ctz));
                },
            );
            bar.inc(1);
        });
    bar.finish();
    // Display error statistics
    println!("\n========== Error Report ==========");
    println!(
        "  - Missing Author ID:       {}",
        error_missing_author_id.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Author Date:     {}",
        error_missing_author_date.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Author TZ:       {}",
        error_missing_author_tz.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer ID:    {}",
        error_missing_committer_id.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer Date:  {}",
        error_missing_committer_date.load(Ordering::Relaxed)
    );
    println!(
        "  - Missing Committer TZ:    {}",
        error_missing_committer_tz.load(Ordering::Relaxed)
    );

    let total_row_errors = error_missing_author_id.load(Ordering::Relaxed)
        + error_missing_author_date.load(Ordering::Relaxed)
        + error_missing_author_tz.load(Ordering::Relaxed)
        + error_missing_committer_id.load(Ordering::Relaxed)
        + error_missing_committer_date.load(Ordering::Relaxed)
        + error_missing_committer_tz.load(Ordering::Relaxed);

    println!("\nTotal Errors: {}", total_row_errors);
    println!("==================================\n");

    flush_to_rocksdb(all_authors, all_committers, data_path)?;
    Ok(())
}
