use anomalous_commits::env;
use anomalous_commits::time_of_day;
use rocksdb::{DB, Options};

fn main() {
    let person = find_by_commits(100).unwrap();
    let mut tod = time_of_day::TimeOfDayProfile::new();
    let commits: Vec<time_of_day::Commit> = person
        .1
        .commits
        .iter()
        .map(|commit| time_of_day::Commit::from_tuple(commit))
        .collect();
    tod.learn(&commits);
    //let distrib = tod.get_distribution();
    let mut anomalous = vec![];
    for commit in commits {
        let score = tod.get_anomaly_score(&commit);
        if score > 0.6 {
            let hour = commit.hour_utc();
            anomalous.push((commit.id, hour, score));
        }
    }
    println!("Found {} anomalous commits.", anomalous.len());
    println!(
        "Anomalous commits for author {} who self committed {} times out of authoring {} commits :\n{:?}\n",
        person.0, person.1.number_of_self_commits, person.1.number_of_commits, anomalous
    );
    //println!("\nDistribution:\n{:?}", distrib);

    // Plot the distribution
    #[cfg(feature = "plotting")]
    let plot_filename = format!("results/tod_profile_{}.png", person.0.replace("/", "_"));
    #[cfg(feature = "plotting")]
    match tod.plot_distribution(
        &plot_filename,
        &format!("Commit Time Distribution - {}", person.0),
    ) {
        Ok(_) => println!("\nPlot saved successfully!"),
        Err(e) => eprintln!("\nx Failed to create plot: {}", e),
    }
}

fn find_by_commits(min_commits: usize) -> Option<(String, env::PersonRecord)> {
    println!(
        "Searching for records with at least {} commits...\n",
        min_commits
    );

    let db = DB::open_for_read_only(&Options::default(), "results/persons.db", false)
        .expect("Couldn't open database");

    let mut iter = db.raw_iterator();
    iter.seek_to_first();

    let mut checked = 0;

    while iter.valid() {
        let key = String::from_utf8(iter.key().expect("key not found").to_vec()).unwrap();
        let value = iter.value().expect("value not found");

        checked += 1;

        // Parse the JSON value
        let json_str = String::from_utf8(value.to_vec()).unwrap();
        let person: env::PersonRecord = serde_json::from_str(&json_str).unwrap();

        if person.number_of_commits >= min_commits {
            println!("Found matching record!\n");
            // println!("\nChecked {} records before finding match.", checked);
            // println!("KEY: {}", key);
            // println!("\nVALUE:");
            // println!("{}", serde_json::to_string_pretty(&person).unwrap());
            return Some((key, person));
        }

        iter.next();
    }

    println!("No records found with at least {} commits.", min_commits);
    println!("Checked {} records.", checked);

    None
}
