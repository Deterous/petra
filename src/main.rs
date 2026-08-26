mod common;
mod exfat;
mod extract;
mod hash;
mod header;
mod rebuild;
mod repair;
mod skeleton;
mod strip;

use std::env;
use std::path::Path;
use std::process::exit;

#[cfg(windows)]
const EXE: &str = "petra.exe";
#[cfg(not(windows))]
const EXE: &str = "petra";

fn usage() {
    println!("PSVita extract/transform/rebuild/analyze (c) Deterous 2026");
    println!();
    println!("Usage:");
    println!("  {EXE} extract <image>     Extracts game files and creates skeleton");
    println!("  {EXE} strip   <image>     Strips image of unique data, saves them separately");
    println!("  {EXE} repair  <image>     Applies sidecar files to a given stripped image");
    println!("  {EXE} rebuild <folder/>   Rebuilds an image from skeleton and folder of files");
    println!("  {EXE} analyze <image>     Scans an image and prints dump image metadata");
    println!("  {EXE} verify  <folder/>   Compares a folder of files with file hashes (.tsv)");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        usage();
        exit(1);
    }

    let path = Path::new(&args[2]);

    let result = match args[1].as_str() {
        "extract" => extract::extract(path),
        "strip" => strip::run(path),
        "repair" => repair::run(path),
        "rebuild" => rebuild::run(path),
        "analyze" => extract::analyze(path),
        "verify" => rebuild::verify(path),
        _ => {
            usage();
            exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        exit(1);
    }
}
