use dotenvy::dotenv;
use flexi_logger::{Cleanup, Criterion, Duplicate, Logger, Naming};
use log::LevelFilter;
use std::env;

fn main() {
    dotenv().ok();
    let dir_orc = env::var("ORC_PATH").expect("ORC_PATH not set");
    Logger::with(LevelFilter::Info)
        .log_to_file(
            flexi_logger::FileSpec::default()
                // .directory("/infres/ir800/rapaport/results/FULL_new/logs")
                // .basename("FULL-all")
                .directory(format!("logs"))
                .basename("anomaly")
                .suffix("log"),
        )
        .rotate(
            Criterion::Size(10_000_000), // Rotate when the file exceeds 10 MB
            Naming::Numbers,             // Use numbers for rotated files
            Cleanup::KeepLogFiles(5),    // Keep at most 5 log files
        )
        .duplicate_to_stderr(Duplicate::Warn) // Duplicate logs to stderr
        .start()
        .unwrap();

    anomalous_commits::extract_author_commits(dir_orc);
}

// 13 columns
// id ok
// msg ok
// author ok
// date ok
// tz ok
// ??
// committer ok
// cdate ok
// ctz ok
// ??
// directory ok
// visit type ok
// ??
