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

    if args.len() < 2 || args.len() > 3 {
        println!("PSVita extract/transform/rebuild/analyze (c) Deterous 2026");
        #[cfg(windows)]
        println!("Usage: petra.exe <path> [license.rif]");
        #[cfg(not(windows))]
        println!("Usage: petra <path> [license.rif]");
        exit(1);
    }

    let path = Path::new(&args[1]);
    let license_path = args.get(2).map(|s| Path::new(s.as_str()));

    let result = if path.is_dir() {
        rebuild::run(path, license_path)
    } else if path.is_file() {
        if license_path.is_some() { Err("ERROR: License file argument is only used for rebuild (directory input)".to_string()) } else { extract::run(path) }
    } else {
        Err("ERROR: Input path does not exist or is not a file/directory".to_string())
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        exit(1);
    }
}
