mod exfat;
mod extract;
mod hash;
mod header;
mod rebuild;
mod skeleton;

use std::env;
use std::path::Path;
use std::process::exit;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 || args.len() > 2 {
        eprintln!("PSV extract/rebuild/analyze (c) Deterous 2026");
        #[cfg(windows)]
        eprintln!("Usage: petra.exe <path>");
        #[cfg(not(windows))]
        eprintln!("Usage: petra <path>");
        exit(1);
    }

    let path = Path::new(&args[1]);

    let result = if path.is_dir() {
        rebuild::run(path)
    } else if path.is_file() {
        extract::run(path)
    } else {
        Err("ERROR: Input path does not exist or is not a file/directory".to_string())
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        exit(1);
    }
}
