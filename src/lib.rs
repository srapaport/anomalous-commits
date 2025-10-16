pub mod env;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use orc_rust::ArrowReaderBuilder;
use rayon::prelude::*;
use std::{
    fs::File,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Flush Authors data to a CSV file
/// 
/// # Arguments
/// * `authors` - Reference to the Authors (CHashMap) containing author information
/// * `output_path` - Path where the CSV file will be written
/// 
/// # Returns
/// Result indicating success or CSV writing error
pub fn flush_authors_to_csv(authors: &env::Authors, output_path: &str) -> Result<(), csv::Error> {
    use std::cell::RefCell;
    use std::rc::Rc;
    
    let mut wtr = csv::Writer::from_path(output_path)?;
    
    // Write CSV header
    wtr.write_record(&[
        "author_id",
        "number_of_commits",
        "time_since_first_commit_ns",
        "commits_list"
    ])?;
    
    // Collect all author data using a temporary vector wrapped in RefCell
    let data_collector = Rc::new(RefCell::new(Vec::new()));
    
    {
        let collector = data_collector.clone();
        authors.retain(|author_id, author_info| {
            collector.borrow_mut().push((
                author_id.clone(),
                author_info.clone(),
            ));
            true // Keep all entries
        });
    }
    
    // Write all records
    for (author_id, author_info) in data_collector.borrow().iter() {
        // Format commits as a semicolon-separated list: "commit_id:timestamp:tz;commit_id:timestamp:tz;..."
        let commits_str = author_info.commits
            .iter()
            .map(|(id, date, tz)| format!("{}:{}:{}", id, date, tz))
            .collect::<Vec<_>>()
            .join(";");
        
        wtr.write_record(&[
            author_id.as_str(),
            author_info.number_of_commits.to_string().as_str(),
            author_info.time_since_first_commit.to_string().as_str(),
            commits_str.as_str(),
        ])?;
    }
    
    wtr.flush()?;
    Ok(())
}

pub fn extract_author_commits(dir_orc: String) {
    let files = std::fs::read_dir(dir_orc)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let error_metadata_not_readable = AtomicUsize::new(0);
    let all_authors = env::Authors::new();
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
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author id
            let Some(authors) = record
                .column(2)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .map(|col| col.iter().collect::<Vec<Option<&[u8]>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author Date
            let Some(dates) = record
                .column(3)
                .as_any()
                .downcast_ref::<arrow::array::TimestampNanosecondArray>()
                .map(|col| col.iter().collect::<Vec<Option<i64>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Author Timezone
            let Some(tzs) = record
                .column(4)
                .as_any()
                .downcast_ref::<arrow::array::Int16Array>()
                .map(|col| col.iter().collect::<Vec<Option<i16>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer id
            let Some(committers) = record
                .column(6)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .map(|col| col.iter().collect::<Vec<Option<&[u8]>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer date
            let Some(cdates) = record
                .column(7)
                .as_any()
                .downcast_ref::<arrow::array::TimestampNanosecondArray>()
                .map(|col| col.iter().collect::<Vec<Option<i64>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Committer Timezone
            let Some(ctzs) = record
                .column(8)
                .as_any()
                .downcast_ref::<arrow::array::Int16Array>()
                .map(|col| col.iter().collect::<Vec<Option<i16>>>())
            else {
                error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // Commit visit type
            // let Some(visits) = record
            //     .column(11)
            //     .as_any()
            //     .downcast_ref::<arrow::array::StringArray>()
            //     .map(|col| col.iter().collect::<Vec<Option<&str>>>())
            // else {
            //     error_metadata_not_readable.fetch_add(1, Ordering::Relaxed);
            //     continue;
            // };
            for row in 0..record.column(0).len() {
                let Some(id) = ids.get(row).and_then(|elem| *elem) else {
                    continue;
                };
                let Some(author) = authors.get(row).and_then(|elem| *elem).map(|bytes| {
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                }) else {
                    continue;
                };
                let Some(date) = dates.get(row).and_then(|elem| *elem) else {
                    continue;
                };
                let Some(tz) = tzs.get(row).and_then(|elem| *elem) else {
                    continue;
                };
                all_authors.upsert(
                    author.clone(),
                    || env::AuthorInfo {
                        nom: None,
                        email: None,
                        number_of_commits: 1,
                        time_since_first_commit: (now - date),
                        commits: vec![(id.to_owned(), date, tz)],
                    },
                    |author_info| {
                        author_info.time_since_first_commit =
                            author_info.time_since_first_commit.max(now - date);
                        author_info.number_of_commits = author_info.number_of_commits + 1;
                        author_info.commits.push((id.to_owned(), date, tz));
                    },
                );
                let Some(committer) = committers.get(row).and_then(|elem| *elem).map(|bytes| {
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                }) else {
                    continue;
                };
                if committer != author {
                    let Some(cdate) = cdates.get(row).and_then(|elem| *elem) else {
                        continue;
                    };
                    let Some(ctz) = ctzs.get(row).and_then(|elem| *elem) else {
                        continue;
                    };
                    all_authors.upsert(committer, ||env::AuthorInfo{
                        nom: None,
                        email: None,
                        number_of_commits: 1,
                        time_since_first_commit: (now - cdate),
                        commits: vec![(id.to_owned(), cdate, ctz)],
                    }, |author_info|{
                        author_info.time_since_first_commit = author_info.time_since_first_commit.max(now-cdate);
                        author_info.number_of_commits = author_info.number_of_commits + 1;
                        author_info.commits.push((id.to_owned(), cdate, ctz));
                    });
                }
            }
        }
        bar.inc(1);
    });
    bar.finish();
    flush_authors_to_csv(&all_authors, "results/authors.csv").unwrap();
    todo!("print amount of errors and add error count in the row loop");
}
