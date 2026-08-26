use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::exfat::{self, CLUSTER_SIZE};
use crate::header;

pub const SONY_MAGIC: &[u8; 32] = b"Sony Computer Entertainment Inc.";
pub const BLACKFIN_MAGIC: &[u8; 16] = b"BlackFin GC Dump";
pub const BLOCK_SIZE: u64 = 512;
pub const HEADER_SKIP: u64 = 512;
pub const UNK_OFFSET: u64 = 0x1C00;
pub const UNK_SIZE: u64 = 0x260;
pub const BLACKFIN_OFFSET: u64 = 0x2000;
pub const BLACKFIN_SIZE: u64 = 0x400;
pub const LIC1_OFFSET: u64 = 0x50;
pub const LIC1_SIZE: u64 = 0x10;
pub const LIC2_OFFSET: u64 = 0xA0;
pub const LIC2_SIZE: u64 = 0x160;

pub fn find_rif(reader: &mut (impl Read + Seek), img_header: &header::ImgHeader, data_offset: u64) -> Result<Option<(u64, u64)>, String> {
    for partition in &img_header.partitions {
        if partition.filesystem != header::FileSystem::ExFat {
            continue;
        }
        let partition_start = partition.offset as u64 * BLOCK_SIZE;
        let partition_size = partition.size as u64 * BLOCK_SIZE;

        let ctx = exfat::parse_seekable(reader, data_offset + partition_start, partition_size)?;
        let cluster_heap_start = ctx.cluster_heap_offset_sectors as u64 * BLOCK_SIZE;

        for file_info in &ctx.files {
            let parts: Vec<&str> = file_info.path.trim_start_matches('/').split('/').collect();
            let is_rif = parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app") && parts[3].ends_with(".rif");

            if is_rif {
                if let Some(&first_cluster) = file_info.chain.first() {
                    let cluster_offset = cluster_heap_start + (first_cluster as u64 - 2) * CLUSTER_SIZE as u64;
                    let abs_offset = data_offset + partition_start + cluster_offset;
                    return Ok(Some((abs_offset, file_info.size)));
                }
            }
        }
    }
    Ok(None)
}

pub fn iter_image_files(dir: &Path, mut f: impl FnMut(&Path) -> Result<(), String>) -> Result<(), String> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("ERROR: Failed to read directory {}: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && matches!(p.extension().and_then(|s| s.to_str()), Some("img" | "psv" | "vci")))
        .collect();
    if files.is_empty() {
        return Err(format!("ERROR: No .img/.psv/.vci files found in {}", dir.display()));
    }
    files.sort();
    let mut had_error = false;
    for path in &files {
        if let Err(e) = f(path) {
            eprintln!("{}", e);
            had_error = true;
        }
    }
    if had_error { Err("ERROR: One or more files failed".to_string()) } else { Ok(()) }
}

pub fn zero_range(file: &mut File, offset: u64, len: u64) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset)).map_err(|e| format!("ERROR: Failed to seek: {}", e))?;
    file.write_all(&vec![0u8; len as usize]).map_err(|e| format!("ERROR: Failed to zero range at 0x{:X}: {}", offset, e))
}

pub fn save_hdr(data: &[u8], basename: &Path) -> Result<(), String> {
    let path = basename.with_extension("hdr");
    fs::write(&path, data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
    println!("Saved: {}", path.display());
    Ok(())
}

pub fn save_unk(data: &[u8], basename: &Path) -> Result<bool, String> {
    if data.iter().any(|&b| b != 0) {
        let path = basename.with_extension("unk");
        fs::write(&path, data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
        println!("Saved: {}", path.display());
        return Ok(true);
    }
    Ok(false)
}

pub fn save_blackfin(data: &[u8], basename: &Path) -> Result<bool, String> {
    if data.len() >= BLACKFIN_MAGIC.len() && &data[..BLACKFIN_MAGIC.len()] == BLACKFIN_MAGIC {
        let path = basename.with_extension("blackfin");
        fs::write(&path, data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
        println!("Saved: {}", path.display());
        return Ok(true);
    }
    Ok(false)
}

pub fn save_rif(data: &[u8], basename: &Path) -> Result<bool, String> {
    let end1 = (LIC1_OFFSET + LIC1_SIZE) as usize;
    let end2 = (LIC2_OFFSET + LIC2_SIZE) as usize;
    if data.len() >= end1.max(end2) && data[LIC1_OFFSET as usize..end1].iter().all(|&b| b == 0) && data[LIC2_OFFSET as usize..end2].iter().all(|&b| b == 0) {
        return Ok(false);
    }
    if data.len() != 0x200 {
        eprintln!("WARNING: Unexpected license file size: {} bytes", data.len());
    }
    let path = basename.with_extension("rif");
    fs::write(&path, data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
    println!("Saved: {}", path.display());
    Ok(true)
}

pub fn check_file_size(file_size: u64, path: &Path) -> Result<(), String> {
    if file_size < 2 * BLOCK_SIZE {
        return Err(format!("ERROR: {} is too small to be a valid psvita image ({} bytes)", path.display(), file_size));
    }
    Ok(())
}

pub fn validate_sony_magic(data: &[u8]) -> bool {
    data.len() >= SONY_MAGIC.len() && &data[..SONY_MAGIC.len()] == SONY_MAGIC
}
