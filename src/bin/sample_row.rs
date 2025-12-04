use anyhow::Result;
use rocksdb::{DB, Options};

fn main() -> Result<()> {
    let db = DB::open_for_read_only(&Options::default(), "results/persons.db", false)?;

    let mut iter = db.raw_iterator();
    iter.seek_to_first();

    if iter.valid() {
        let key = iter
            .key()
            .ok_or_else(|| anyhow::anyhow!("Failed to get key"))?;
        let value = iter
            .value()
            .ok_or_else(|| anyhow::anyhow!("Failed to get value"))?;

        println!("KEY: {}", String::from_utf8(key.to_vec())?);
        println!("\nVALUE:");
        println!("{}", String::from_utf8(value.to_vec())?);
    } else {
        println!("No data in database");
    }

    Ok(())
}
