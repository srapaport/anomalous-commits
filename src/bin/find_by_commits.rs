use anyhow::Result;
use rocksdb::{DB, Options};
use serde_json::Value;
use std::env;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let min_commits: usize = args[1].parse()?;

    println!(
        "Searching for records with at least {} commits...\n",
        min_commits
    );

    let db = DB::open_for_read_only(&Options::default(), "results/persons.db", false)?;

    let mut iter = db.raw_iterator();
    iter.seek_to_first();

    let mut found = false;
    let mut checked = 0;

    while iter.valid() {
        let key = iter
            .key()
            .ok_or_else(|| anyhow::anyhow!("Failed to get key"))?;
        let value = iter
            .value()
            .ok_or_else(|| anyhow::anyhow!("Failed to get value"))?;

        checked += 1;

        // Parse the JSON value
        let json_str = String::from_utf8(value.to_vec())?;
        let person: Value = serde_json::from_str(&json_str)?;

        // Check if number_of_commits >= min_commits
        if let Some(num_commits) = person["number_of_commits"].as_u64() {
            if num_commits as usize >= min_commits {
                println!("Found matching record!");
                println!("\nChecked {} records before finding match.", checked);
                println!("KEY: {}", String::from_utf8(key.to_vec())?);
                println!("\nVALUE:");
                println!("{}", serde_json::to_string_pretty(&person)?);
                found = true;
                break;
            }
        }

        iter.next();
    }

    if !found {
        println!("No records found with at least {} commits.", min_commits);
        println!("Checked {} records.", checked);
    }

    Ok(())
}
