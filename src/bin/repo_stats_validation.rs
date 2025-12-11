use anyhow::Result;
use dotenvy::dotenv;
use flexi_logger::{Cleanup, Criterion, Duplicate, Logger, Naming};
use log::LevelFilter;
use std::env;
use swh_graph::graph::*;
use swh_graph::mph::DynMphf;

fn main() -> Result<()> {
    dotenv()?;
    let graph_basename = env::var("GRAPH_BASENAME").expect("GRAPH_BASENAME not set");
    Logger::with(LevelFilter::Info)
        .log_to_file(
            flexi_logger::FileSpec::default()
                // .directory("/infres/ir800/rapaport/results/FULL_new/logs")
                // .basename("FULL-all")
                .directory(format!("logs"))
                .basename("repo_stats_validation")
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
    let graph = SwhBidirectionalGraph::new(graph_basename)?
        .load_all_properties::<DynMphf>()?
        .load_forward_labels()?
        .load_backward_labels()?;

    let db_path = "data/RepositoryStats_validation.db";
    let validation_repos = vec![
        (
            "https://github.com/atom-minimap/minimap",
            vec!["swh:1:rev:16c11d82b889ce1260342e4fa7d6d1905c0fde45"],
        ),
        // (
        //     "https://github.com/greatsuspender/thegreatsuspender",
        //     vec![],
        // ),
        // ("https://github.com/gentoo/gentoo", vec![]),
        (
            "https://github.com/RIAEvangelist/node-ipc",
            vec![
                "swh:1:rev:847047cf7f81ab08352038b2204f0e7633449580",
                "swh:1:rev:6e344066a0464814a27fbd7ca8422f473956a803",
            ],
        ),
        (
            "https://github.com/php/php-src",
            vec![
                "swh:1:rev:c730aa26bd52829a49f2ad284b181b7e82a68d7d",
                "swh:1:rev:2b0f239b211c7544ebc7a4cd2c977a5b7a11ed8a",
            ],
        ),
        (
            "https://github.com/dominictarr/event-stream",
            vec!["swh:1:rev:e3163361fed01384c986b9b4c18feb1fc42b8285"],
        ),
        ("https://github.com/Marak/colors.js", vec![]),
        // (
        //     "https://github.com/gentoo/systemd",
        //     vec!["swh:1:rev:50e3544d11046408b610b955591319b1df949311"],
        // ),
        // ("https://github.com/TesyarRAz/KeseQul-Desktop-Alpha", vec![]),
        // ("https://github.com/SebasR16/BdProyecto", vec![]),
        // ("https://github.com/Sliray/Secuencia-Numerica", vec![]),
        // ("https://github.com/KosimCorp/RatingVoteEPITECH", vec![]),
        // ("https://github.com/george-bennett/V2Mp3Player", vec![]),
        // ("https://github.com/KosimCorp/Kosim-Framework", vec![]),
        // ("https://github.com/SebasR16/Punto-de-venta", vec![]),
        // ("https://github.com/BarbosaO/2D-Physics-Simulations", vec![]),
        // ("https://github.com/SierraBrandt/GuessTheAnimal", vec![]),
        // ("https://github.com/callmehetch/PacmanGame", vec![]),
        // ("https://github.com/FelixGtz99/ProyectoGerundio", vec![]),
    ];
    anomalous_commits::build_repo_stats(&graph, db_path, Some(&validation_repos))?;
    Ok(())
}
