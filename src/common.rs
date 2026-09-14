use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crc32fast::Hasher as Crc32Hasher;
use sha1::Digest as Sha1Digest;
use sha1::Sha1;
use sha2::{Digest as Sha2Digest, Sha256};

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

pub fn save_footer(data: &[u8], basename: &Path) -> Result<bool, String> {
    if data.iter().any(|&b| b != 0) {
        let path = basename.with_extension("ftr");
        fs::write(&path, data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
        println!("Saved: {}", path.display());
        return Ok(true);
    }
    Ok(false)
}

pub fn hash_file(file: &mut File, file_size: u64) -> Result<(u32, String, String, String), String> {
    print!("Hashing...");
    std::io::stdout().flush().ok();
    file.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: {}", e))?;
    let mut crc = Crc32Hasher::new();
    let mut md5_ctx = md5::Context::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut remaining = file_size;
    while remaining > 0 {
        let to_read = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read for hashing: {}", e))?;
        crc.update(&buf[..to_read]);
        md5_ctx.consume(&buf[..to_read]);
        Sha1Digest::update(&mut sha1, &buf[..to_read]);
        Sha2Digest::update(&mut sha256, &buf[..to_read]);
        remaining -= to_read as u64;
    }
    let crc_val = crc.finalize();
    let to_hex = |b: &[u8]| b.iter().fold(String::new(), |mut s, x| { use std::fmt::Write; write!(s, "{:02x}", x).unwrap(); s });
    let md5_str = to_hex(&md5_ctx.compute().0);
    let sha1_str = to_hex(&Sha1Digest::finalize(sha1));
    let sha256_str = to_hex(&Sha2Digest::finalize(sha256));
    println!("Done");
    Ok((crc_val, md5_str, sha1_str, sha256_str))
}

pub fn save_dat(roms: &[(&str, u64, u32, &str, &str, &str)], path: &Path) -> Result<(), String> {
    let xml = roms.iter().map(|(name, size, crc, md5, sha1, sha256)|
        format!("<rom name=\"{}\" size=\"{}\" crc=\"{:08x}\" md5=\"{}\" sha1=\"{}\" sha256=\"{}\"/>", name, size, crc, md5, sha1, sha256)
    ).collect::<Vec<_>>().join("\n");
    fs::write(path, xml).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
    println!("Saved: {}", path.display());
    Ok(())
}

pub fn read_dat(basename: &Path) -> Result<Option<(String, u64)>, String> {
    let path = basename.with_extension("dat");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("ERROR: Failed to read {}: {}", path.display(), e))?;
    let parse_attr = |attr: &str| -> Option<&str> {
        let needle = format!("{}=\"", attr);
        let start = content.find(needle.as_str())? + needle.len();
        let end = content[start..].find('"')? + start;
        Some(&content[start..end])
    };
    let name = parse_attr("name").ok_or_else(|| format!("ERROR: Could not parse name from {}", path.display()))?.to_string();
    let size: u64 = parse_attr("size").ok_or_else(|| format!("ERROR: Could not parse size from {}", path.display()))?.parse().map_err(|_| format!("ERROR: Invalid size in {}", path.display()))?;
    Ok(Some((name, size)))
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
