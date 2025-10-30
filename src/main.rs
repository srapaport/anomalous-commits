use anyhow::Result;
use dotenvy::dotenv;
use flexi_logger::{Cleanup, Criterion, Duplicate, Logger, Naming};
use log::LevelFilter;
use std::env;

fn main() -> Result<()> {
    dotenv().ok();
    let graph_path = env::var("GRAPH_BASENAME").expect("GRAPH_BASENAME not set");
    let results_repo_stats = env::var("DATABASE_REPO_STATS").expect("DATABASE_REPO_STATS not set");
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

    anomalous_commits::outliers::parsing_graph_repositories(graph_path, results_repo_stats)?;
    Ok(())
}
