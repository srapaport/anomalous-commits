pub mod env;
pub mod time_of_day;
pub mod outliers;

use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use orc_rust::ArrowReaderBuilder;
use rayon::prelude::*;
use rocksdb::{DB, Options};
use std::{
    fs::File,
    sync::atomic::{AtomicUsize, Ordering},
};

pub fn flush_to_rocksdb(authors: env::Authors, committers: env::Committers, db_path: &str) -> Result<(), rocksdb::Error> {
    // Delete the database if it exists to start fresh
    if std::path::Path::new(db_path).exists() {
        DB::destroy(&Options::default(), db_path)?;
    }

    // Open/create RocksDB database
    let mut opts = Options::default();
    opts.create_if_missing(true);
    let db = DB::open(&opts, db_path)?;

    for (author_id, author_info) in authors.into_iter() {
        let person_record = env::PersonRecord {
            person_type: env::PersonType::Author,
            nom: author_info.nom,
            email: author_info.email,
            number_of_commits: author_info.number_of_commits,
            number_of_self_commits: author_info.number_of_self_commits,
            time_since_first_commit: author_info.time_since_first_commit,
            commits: author_info.commits,
        };
        
        let value = serde_json::to_vec(&person_record)
            .expect("Failed to serialize author info");
        
        // Store in RocksDB with "author:<author_id>" as key
        let key = format!("author:{}", author_id);
        db.put(key.as_bytes(), value)?;
    }

    for (committer_id, committer_info) in committers.into_iter() {
        let person_record = env::PersonRecord {
            person_type: env::PersonType::Committer,
            nom: committer_info.nom,
            email: committer_info.email,
            number_of_commits: committer_info.number_of_commits,
            number_of_self_commits: committer_info.number_of_self_commits,
            time_since_first_commit: committer_info.time_since_first_commit,
            commits: committer_info.commits,
        };

        let value = serde_json::to_vec(&person_record)
            .expect("Failed to serialize committer info");
        
        // Store in RocksDB with "committer:<committer_id>" as key
        let key = format!("committer:{}", committer_id);
        db.put(key.as_bytes(), value)?;
    }

    db.flush()?;
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
                        if self_committed{
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
                        time_since_first_commit: (now - cdate),
                        commits: vec![(id.to_owned(), cdate, ctz)],
                    },
                    |committer_info| {
                        committer_info.time_since_first_commit =
                            committer_info.time_since_first_commit.max(now - cdate);
                        committer_info.number_of_commits = committer_info.number_of_commits + 1;
                        if self_committed{
                            committer_info.number_of_self_commits = committer_info.number_of_self_commits + 1;
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
    println!("  - Commit IDs:           {}", error_commit_ids.load(Ordering::Relaxed));
    println!("  - Author IDs:           {}", error_author_ids.load(Ordering::Relaxed));
    println!("  - Author Dates:         {}", error_author_dates.load(Ordering::Relaxed));
    println!("  - Author Timezones:     {}", error_author_timezones.load(Ordering::Relaxed));
    println!("  - Committer IDs:        {}", error_committer_ids.load(Ordering::Relaxed));
    println!("  - Committer Dates:      {}", error_committer_dates.load(Ordering::Relaxed));
    println!("  - Committer Timezones:  {}", error_committer_timezones.load(Ordering::Relaxed));
    
    let total_column_errors = error_commit_ids.load(Ordering::Relaxed)
        + error_author_ids.load(Ordering::Relaxed)
        + error_author_dates.load(Ordering::Relaxed)
        + error_author_timezones.load(Ordering::Relaxed)
        + error_committer_ids.load(Ordering::Relaxed)
        + error_committer_dates.load(Ordering::Relaxed)
        + error_committer_timezones.load(Ordering::Relaxed);
    
    println!("\nRow-level errors (missing data in specific rows):");
    println!("  - Missing Commit ID:       {}", error_missing_commit_id.load(Ordering::Relaxed));
    println!("  - Missing Author ID:       {}", error_missing_author_id.load(Ordering::Relaxed));
    println!("  - Missing Author Date:     {}", error_missing_author_date.load(Ordering::Relaxed));
    println!("  - Missing Author TZ:       {}", error_missing_author_tz.load(Ordering::Relaxed));
    println!("  - Missing Committer ID:    {}", error_missing_committer_id.load(Ordering::Relaxed));
    println!("  - Missing Committer Date:  {}", error_missing_committer_date.load(Ordering::Relaxed));
    println!("  - Missing Committer TZ:    {}", error_missing_committer_tz.load(Ordering::Relaxed));
    
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
    println!("  - Grand Total:  {}", total_column_errors + total_row_errors);
    println!("==================================\n");
    
    // Transfer ownership to flush_to_rocksdb - data will be consumed and freed
    flush_to_rocksdb(all_authors, all_committers, "results/persons.db").unwrap();
}
