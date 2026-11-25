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
    let hdr_path = common::companion_path(&basename, "hdr");
    let unk_path = common::companion_path(&basename, "unk");
    let blackfin_path = common::companion_path(&basename, "blackfin");
    let rif_path = common::companion_path(&basename, "rif");
    let has_rif_sidecar = rif_path.exists()
        || fs::read_dir(basename.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new(".")))
            .map_err(|e| format!("ERROR: Failed to read directory: {}", e))?
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                let name = entry.file_name();
                entry.path().is_file()
                    && name.to_str().map_or(false, |name| {
                        name.ends_with(".rif")
                            && name.starts_with(format!("{}.", basename.file_name().unwrap_or_default().to_string_lossy()).as_str())
                    })
            });
    let footer_path = common::companion_path(&basename, "ftr");
    let dat = common::read_dat(&basename)?;

    let mut file = File::options().read(true).write(true).open(path).map_err(|e| format!("ERROR: Failed to open {}: {}", path.display(), e))?;
    let mut file_size = file.metadata().map_err(|e| format!("ERROR: {}", e))?.len();
    common::check_file_size(file_size, path)?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|e| format!("ERROR: Failed to read magic: {}", e))?;
    let has_header = &magic == b"PSV\0" || &magic == b"VCI\0";
    let mut data_offset: u64 = if has_header { HEADER_SKIP } else { 0 };

    if !hdr_path.exists() && !unk_path.exists() && !blackfin_path.exists() && !has_rif_sidecar && !footer_path.exists() {
        let needs_repair = dat.as_ref().map_or(false, |(original_name, target_size)| {
            file_size < *target_size || path.with_file_name(original_name.as_str()) != path
        });
        if !needs_repair {
            println!("Nothing to repair: {}", path.display());
            return Ok(());
        }
    }

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

    if has_rif_sidecar {
        let data_size = file_size - data_offset;
        file.seek(SeekFrom::Start(data_offset)).map_err(|e| format!("ERROR: {}", e))?;
        let img_header = header::parse(&mut file, data_size)?;
        let rifs = common::find_rifs(&mut file, &img_header, data_offset)?;
        if rifs.is_empty() {
            return Err("ERROR: No .rif file found in image filesystem".to_string());
        }

        for (rif_file, offset, size) in rifs {
            let parts: Vec<&str> = rif_file.trim_start_matches('/').split('/').collect();
            let expected_sidecar = if parts[1].eq_ignore_ascii_case("app") {
                rif_path.clone()
            } else {
                common::companion_path(&basename, &format!("{}.rif", parts[2]))
            };
            if !expected_sidecar.exists() {
                continue;
            }

            let rif_data = fs::read(&expected_sidecar).map_err(|e| format!("ERROR: Failed to read {}: {}", expected_sidecar.display(), e))?;
            if rif_data.len() != 0x200 {
                eprintln!("WARNING: {} is not a valid license file - skipping", expected_sidecar.display());
                continue;
            }
            if size != 0x200 {
                eprintln!("WARNING: {} is {} bytes, expected 512 - skipping", rif_file, size);
                continue;
            }
            file.seek(SeekFrom::Start(offset)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&rif_data).map_err(|e| format!("ERROR: Failed to write rif: {}", e))?;
            println!("Applied: {} -> {}", expected_sidecar.display(), rif_file);
        }
    }

    if footer_path.exists() {
        let footer = fs::read(&footer_path).map_err(|e| format!("ERROR: Failed to read {}: {}", footer_path.display(), e))?;
        file.seek(SeekFrom::End(0)).map_err(|e| format!("ERROR: {}", e))?;
        file.write_all(&footer).map_err(|e| format!("ERROR: Failed to write footer: {}", e))?;
        println!("Applied: {}", footer_path.display());
    } else if let Some((_, target_size)) = dat.as_ref() {
        let current_size = file.metadata().map_err(|e| format!("ERROR: {}", e))?.len();
        if current_size < *target_size {
            file.seek(SeekFrom::End(0)).map_err(|e| format!("ERROR: {}", e))?;
            let pad = target_size - current_size;
            let zeros = vec![0u8; pad.min(8 * 1024 * 1024) as usize];
            let mut remaining = pad;
            while remaining > 0 {
                let n = remaining.min(zeros.len() as u64) as usize;
                file.write_all(&zeros[..n]).map_err(|e| format!("ERROR: Failed to pad file: {}", e))?;
                remaining -= n as u64;
            }
            println!("Padded to {} bytes: {}", target_size, path.display());
        }
    }

    drop(file);
    if let Some((original_name, _)) = dat {
        let orig_path = path.with_file_name(&original_name);
        if orig_path != path {
            fs::rename(path, &orig_path).map_err(|e| format!("ERROR: Failed to rename to {}: {}", orig_path.display(), e))?;
            println!("Renamed: {} -> {}", path.display(), orig_path.display());
        }
    }

    println!("Done: {}", path.display());
    Ok(())
}
