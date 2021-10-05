use std::path::PathBuf;

#[path = "src/opts.rs"]
mod opts;

fn main() {
    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(out_dir) => out_dir,
        None => {
            panic!(
                "OUT_DIR environment variable was not set. \
                This should have been set by Cargo. \
                As a result, man-pages cannot be generated."
            );
        }
    };
    let out_dir = PathBuf::from(out_dir);
    let man_dir = out_dir.join("man1");
    std::fs::create_dir_all(&man_dir).unwrap();

    // Note that writing the man-pages into `OUT_DIR` doesn't do anything by
    // itself. We would need support from the system package manager to move
    // them into place on the target system.
    opts::write_man_pages(&man_dir).unwrap();
}
