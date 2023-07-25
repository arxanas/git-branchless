use clap::Parser;
use scm_record::scm_diff_editor::{scm_diff_editor_main, Opts};

fn main() {
    let opts = Opts::parse();
    match scm_diff_editor_main(opts) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
