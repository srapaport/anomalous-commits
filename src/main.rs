use std::fs::File;
use orc_rust::ArrowReaderBuilder;

fn main() {
    let file = File::open("/Users/raps/Documents/02_Work/dataset.nosync/2025-05-18-popular-1k/orc/revision/20250903_232723_00127_avcv4_f35f0e81-0717-4e62-8157-100a583322a1").unwrap();
    let reader = ArrowReaderBuilder::try_new(file).unwrap().build();
    let record_batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
    for record in record_batches{
        let Some(generic_ids) = record.column(0).as_any().downcast_ref::<arrow::array::StringArray>() else{ break };
        let ids = generic_ids.iter().collect::<Vec<Option<&str>>>();
        let Some(generic_messages) = record.column(1).as_any().downcast_ref::<arrow::array::BinaryArray>() else{ break };
        let messages = generic_messages.iter().collect::<Vec<Option<&[u8]>>>();
        let _authors = record.column(2);
        for row in 0..record.column(0).len(){
            let Some(elem) = ids.get(row) else{continue};
            let Some(id) = *elem else { continue };
            println!("commit id: {}", id);
            let Some(elem) = messages.get(row) else{continue};
            let Some(bytes) = *elem else{continue};
            let Ok(msg) = String::from_utf8(bytes.to_vec()) else {continue};
            println!("commit msg: {}", msg);
            break;
        }
        break;
    }
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