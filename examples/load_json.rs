#![warn(clippy::all, clippy::as_conversions)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::fs::File;

use scm_record::{EventSource, RecordError, RecordState, Recorder, SelectedContents};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_filename = args.get(1).expect("expected JSON dump as first argument");
    let json_file = File::open(json_filename).expect("opening JSON file");
    let record_state: RecordState =
        serde_json::from_reader(json_file).expect("deserializing state");

    let recorder = Recorder::new(record_state, EventSource::Crossterm);
    let result = recorder.run();
    match result {
        Ok(result) => {
            let RecordState { files } = result;
            for file in files {
                println!("--- Path {:?} final lines: ---", file.path);
                let (selected, _unselected) = file.get_selected_contents();
                print!(
                    "{}",
                    match &selected {
                        SelectedContents::Absent => "<absent>\n".to_string(),
                        SelectedContents::Unchanged => "<unchanged\n>".to_string(),
                        SelectedContents::Binary {
                            old_description: _,
                            new_description: None,
                        } => "<binary>\n".to_string(),
                        SelectedContents::Binary {
                            old_description: _,
                            new_description: Some(description),
                        } => format!("<binary description={description}>\n"),
                        SelectedContents::Present { contents } => contents.clone(),
                    }
                );
            }
        }
        Err(RecordError::Cancelled) => println!("Cancelled!\n"),
        Err(err) => {
            println!("Error: {err}");
        }
    }
}
