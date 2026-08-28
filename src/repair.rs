use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::common::{self, BLACKFIN_OFFSET, BLACKFIN_SIZE, HEADER_SKIP, UNK_OFFSET, UNK_SIZE};
use crate::header;

pub fn run(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return common::iter_image_files(path, run_single);
    }
    run_single(path)
}

fn run_single(path: &Path) -> Result<(), String> {
    let basename = path.with_extension("");
    let hdr_path = basename.with_extension("hdr");
    let unk_path = basename.with_extension("unk");
    let blackfin_path = basename.with_extension("blackfin");
    let rif_path = basename.with_extension("rif");

    let mut file = File::options().read(true).write(true).open(path).map_err(|e| format!("ERROR: Failed to open {}: {}", path.display(), e))?;
    let mut file_size = file.metadata().map_err(|e| format!("ERROR: {}", e))?.len();
    common::check_file_size(file_size, path)?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|e| format!("ERROR: Failed to read magic: {}", e))?;
    let has_header = &magic == b"PSV\0" || &magic == b"VCI\0";
    let mut data_offset: u64 = if has_header { HEADER_SKIP } else { 0 };

    if hdr_path.exists() {
        let hdr = fs::read(&hdr_path).map_err(|e| format!("ERROR: Failed to read {}: {}", hdr_path.display(), e))?;
        if hdr.len() != HEADER_SKIP as usize {
            eprintln!("WARNING: {} is {} bytes, expected {} - skipping", hdr_path.display(), hdr.len(), HEADER_SKIP);
        } else if has_header {
            file.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&hdr).map_err(|e| format!("ERROR: Failed to write hdr: {}", e))?;
            println!("Applied: {}", hdr_path.display());
        } else {
            let mut buf = vec![0u8; 8 * 1024 * 1024];
            file_size += HEADER_SKIP;
            file.set_len(file_size).map_err(|e| format!("ERROR: Failed to extend file: {}", e))?;
            let mut read_pos = file_size - HEADER_SKIP;
            let mut write_pos = file_size;
            while read_pos > 0 {
                let chunk = read_pos.min(8 * 1024 * 1024) as usize;
                read_pos -= chunk as u64;
                write_pos -= chunk as u64;
                file.seek(SeekFrom::Start(read_pos)).map_err(|e| format!("ERROR: {}", e))?;
                file.read_exact(&mut buf[..chunk]).map_err(|e| format!("ERROR: Failed to read during hdr prepend: {}", e))?;
                file.seek(SeekFrom::Start(write_pos)).map_err(|e| format!("ERROR: {}", e))?;
                file.write_all(&buf[..chunk]).map_err(|e| format!("ERROR: Failed to write during hdr prepend: {}", e))?;
            }
            file.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&hdr).map_err(|e| format!("ERROR: Failed to write hdr: {}", e))?;
            data_offset = HEADER_SKIP;
            println!("Applied: {}", hdr_path.display());
        }
    }

    if unk_path.exists() {
        let unk = fs::read(&unk_path).map_err(|e| format!("ERROR: Failed to read {}: {}", unk_path.display(), e))?;
        if unk.len() != UNK_SIZE as usize {
            eprintln!("WARNING: {} is {} bytes, expected {} - skipping", unk_path.display(), unk.len(), UNK_SIZE);
        } else {
            file.seek(SeekFrom::Start(data_offset + UNK_OFFSET)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&unk).map_err(|e| format!("ERROR: Failed to write unk: {}", e))?;
            println!("Applied: {}", unk_path.display());
        }
    }

    if blackfin_path.exists() {
        let blackfin = fs::read(&blackfin_path).map_err(|e| format!("ERROR: Failed to read {}: {}", blackfin_path.display(), e))?;
        if blackfin.len() != BLACKFIN_SIZE as usize {
            eprintln!("WARNING: {} is {} bytes, expected {} - skipping", blackfin_path.display(), blackfin.len(), BLACKFIN_SIZE);
        } else {
            file.seek(SeekFrom::Start(data_offset + BLACKFIN_OFFSET)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&blackfin).map_err(|e| format!("ERROR: Failed to write blackfin: {}", e))?;
            println!("Applied: {}", blackfin_path.display());
        }
    }

    if rif_path.exists() {
        let rif = fs::read(&rif_path).map_err(|e| format!("ERROR: Failed to read {}: {}", rif_path.display(), e))?;

        if rif.len() != 0x200 {
            eprintln!("WARNING: {} is not a valid license file - skipping", rif_path.display());
        } else {
            let data_size = file_size - data_offset;
            file.seek(SeekFrom::Start(data_offset)).map_err(|e| format!("ERROR: {}", e))?;
            let img_header = header::parse(&mut file, data_size)?;

            let (rif_abs_offset, _) = common::find_rif(&mut file, &img_header, data_offset)?.ok_or("ERROR: No .rif file found in image filesystem")?;

            file.seek(SeekFrom::Start(rif_abs_offset)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&rif).map_err(|e| format!("ERROR: Failed to write rif: {}", e))?;
            println!("Applied: {}", rif_path.display());
        }
    }

    if !hdr_path.exists() && !unk_path.exists() && !blackfin_path.exists() && !rif_path.exists() {
        println!("Nothing to repair: {}", path.display());
        return Ok(());
    }

    println!("Done: {}", path.display());
    Ok(())
}
