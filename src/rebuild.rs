use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::exfat::{self, BYTES_PER_SECTOR, CLUSTER_SIZE};
use crate::hash::{self, HashEntry};
use crate::header::{self, FileSystem};
use crate::skeleton;

struct ScannedFile {
    path: PathBuf,
    size: u64,
}

pub fn run(input_dir: &Path, license_path: Option<&Path>) -> Result<(), String> {
    let parent = input_dir.parent().unwrap_or(Path::new("."));
    let name = input_dir.file_name().ok_or("Cannot determine folder name")?.to_string_lossy();

    let skeleton_zst_path = parent.join(format!("{}.skeleton.zst", name));
    let skeleton_raw_path = parent.join(format!("{}.skeleton", name));
    let hash_path = parent.join(format!("{}.tsv", name));
    let output_path = parent.join(format!("{}.img", name));

    if output_path.exists() {
        return Err(format!("ERROR: Output file already exists: {}", output_path.display()));
    }

    if !hash_path.exists() {
        return Err(format!("ERROR: Hash file not found: {}", hash_path.display()));
    }

    let hash_entries = hash::read_hash_file(&hash_path)?;

    let expected_sizes: HashSet<u64> = hash_entries.iter().map(|e| e.size).collect();

    let mut scanned_files = Vec::new();
    scan_recursive(input_dir, &mut scanned_files)?;

    let mut matches = match_files(&scanned_files, &hash_entries, &expected_sizes)?;

    if let Some(lic_path) = license_path {
        let lic_meta = std::fs::metadata(lic_path).map_err(|e| format!("ERROR: Failed to read license file {}: {}", lic_path.display(), e))?;
        let lic_size = lic_meta.len();

        for (idx, entry) in hash_entries.iter().enumerate() {
            let parts: Vec<&str> = entry.path.trim_start_matches('/').split('/').collect();
            let is_license_rif = parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app") && parts[3].ends_with(".rif");
            if entry.size == lic_size && is_license_rif {
                println!("Using license override for: {}", entry.path);
                matches.insert(idx, lic_path.to_path_buf());
            }
        }
    }

    if matches.len() != hash_entries.len() {
        let mut unmatched = Vec::new();
        for (idx, entry) in hash_entries.iter().enumerate() {
            if !matches.contains_key(&idx) {
                unmatched.push(entry);
            }
        }
        let mut msg = format!("ERROR: Missing {} file(s) for rebuild:\n", unmatched.len());
        for entry in &unmatched {
            msg.push_str(&format!("  sha256={} size={} ({})\n", entry.sha256, entry.size, entry.path));
        }
        return Err(msg.trim_end().to_string());
    }

    let use_raw_skeleton = if skeleton_raw_path.exists() {
        true
    } else if skeleton_zst_path.exists() {
        false
    } else {
        return Err(format!("ERROR: Skeleton file not found: {}", skeleton_zst_path.display()));
    };

    let file_size = if use_raw_skeleton { skeleton::read_image_size_raw(&skeleton_raw_path)? } else { skeleton::read_image_size(&skeleton_zst_path)? };

    hash::validate_hash_bounds(&hash_entries, file_size)?;

    if use_raw_skeleton {
        std::fs::copy(&skeleton_raw_path, &output_path).map_err(|e| format!("ERROR: Failed to copy skeleton to output: {}", e))?;
    } else {
        skeleton::decompress_skeleton(&skeleton_zst_path, &output_path).map_err(|e| format!("ERROR: Failed to decompress skeleton {}: {}", skeleton_zst_path.display(), e))?;
    }

    let img_header = {
        let mut f = File::open(&output_path).map_err(|e| format!("ERROR: Failed to open output to read header: {}", e))?;
        header::parse(&mut f, file_size)?
    };

    if let Err(e) = insert_img_partitions(&output_path, &img_header, parent) {
        let _ = std::fs::remove_file(&output_path);
        return Err(e);
    }

    if let Err(e) = insert_files(&output_path, &hash_entries, &matches) {
        let _ = std::fs::remove_file(&output_path);
        return Err(e);
    }

    println!("Rebuilt: {}", output_path.display());

    Ok(())
}

fn insert_img_partitions(output_path: &Path, img_header: &header::ImgHeader, input_dir: &Path) -> Result<(), String> {
    let mut partitions: Vec<header::Partition> = img_header.partitions.clone();
    partitions.sort_by_key(|p| p.offset);
    let names = header::partition_names(&partitions, |fs| !matches!(fs, FileSystem::ExFat));

    let mut output = std::fs::OpenOptions::new().write(true).open(output_path).map_err(|e| format!("ERROR: Failed to open output for FAT16 insertion: {}", e))?;

    for (partition, name) in partitions.iter().zip(names.iter()) {
        if partition.filesystem == FileSystem::ExFat {
            continue;
        }
        let name = match name {
            Some(n) => n,
            None => continue,
        };
        let img_path = input_dir.join(format!("{}.img", name));
        let partition_offset = partition.offset as u64 * 512;
        let partition_size = partition.size as u64 * 512;

        if !img_path.exists() {
            println!("Partition {} not found ({}), leaving zeroed", name, img_path.display());
            continue;
        }

        let mut src = File::open(&img_path).map_err(|e| format!("ERROR: Failed to open {}: {}", img_path.display(), e))?;
        let src_size = src.metadata().map_err(|e| format!("ERROR: Failed to stat {}: {}", img_path.display(), e))?.len();
        if src_size != partition_size {
            return Err(format!("ERROR: {} is {} bytes but partition expects {} bytes", img_path.display(), src_size, partition_size));
        }

        output.seek(SeekFrom::Start(partition_offset)).map_err(|e| format!("ERROR: Failed to seek to partition offset: {}", e))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut remaining = partition_size;
        while remaining > 0 {
            let to_read = remaining.min(buf.len() as u64) as usize;
            src.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read {}: {}", img_path.display(), e))?;
            output.write_all(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write partition: {}", e))?;
            remaining -= to_read as u64;
        }
        println!("Inserted {} partition from {}", name, img_path.display());
    }
    Ok(())
}

fn scan_recursive(dir: &Path, files: &mut Vec<ScannedFile>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("ERROR: Failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("ERROR: Failed to read entry in {}: {}", dir.display(), e))?;
        let path = entry.path();
        if path.is_dir() {
            scan_recursive(&path, files)?;
        } else if path.is_file() {
            let metadata = std::fs::metadata(&path).map_err(|e| format!("ERROR: Failed to read metadata for {}: {}", path.display(), e))?;
            files.push(ScannedFile { path, size: metadata.len() });
        }
    }
    Ok(())
}

fn match_files(scanned: &[ScannedFile], entries: &[HashEntry], expected_sizes: &HashSet<u64>) -> Result<HashMap<usize, PathBuf>, String> {
    let mut lookup: HashMap<(&str, u64), Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        lookup.entry((&entry.sha256, entry.size)).or_default().push(idx);
    }

    let mut matches: HashMap<usize, PathBuf> = HashMap::new();

    for file in scanned {
        if !expected_sizes.contains(&file.size) {
            continue;
        }

        let sha256 = hash_file(&file.path)?;

        if let Some(entry_indices) = lookup.get(&(sha256.as_str(), file.size)) {
            for &entry_idx in entry_indices {
                matches.insert(entry_idx, file.path.clone());
            }
        }
    }

    Ok(matches)
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("ERROR: Failed to open {}: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("ERROR: Failed to read {}: {}", path.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in hash.iter() {
        use std::fmt::Write;
        write!(hex, "{:02x}", byte).unwrap();
    }
    Ok(hex)
}

fn insert_files(output_path: &Path, entries: &[HashEntry], matches: &HashMap<usize, PathBuf>) -> Result<(), String> {
    let file = std::fs::OpenOptions::new().read(true).write(true).open(output_path).map_err(|e| format!("ERROR: Failed to open img for inserting: {}", e))?;
    let file_size = file.metadata().map_err(|e| format!("ERROR: Failed to get output metadata: {}", e))?.len();
    let mut rw = BufWriter::new(file);

    rw.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: Failed to seek to header: {}", e))?;
    let mut header_buf = [0u8; 512];
    rw.get_mut().read_exact(&mut header_buf).map_err(|e| format!("ERROR: Failed to read header: {}", e))?;

    let img_header = header::parse(&mut &header_buf[..], file_size)?;

    let cluster_size = CLUSTER_SIZE as u64;
    let mut offset_to_chain: HashMap<u64, (u64, u32, Vec<u32>)> = HashMap::new();

    for partition in &img_header.partitions {
        if partition.filesystem != header::FileSystem::ExFat {
            continue;
        }
        let partition_start = partition.offset as u64 * 512;
        let partition_size = partition.size as u64 * 512;

        let ctx = exfat::parse_seekable(rw.get_mut(), partition_start, partition_size)?;
        let heap_byte_offset = partition_start + ctx.cluster_heap_offset_sectors as u64 * BYTES_PER_SECTOR as u64;

        for file_info in &ctx.files {
            if let Some(&first_cluster) = file_info.chain.first() {
                let first_cluster_byte = heap_byte_offset + (first_cluster as u64 - 2) * cluster_size;
                let sector_offset = first_cluster_byte / 512;
                offset_to_chain.insert(sector_offset, (partition_start, ctx.cluster_heap_offset_sectors, file_info.chain.clone()));
            }
        }
    }

    for (idx, entry) in entries.iter().enumerate() {
        let file_path = &matches[&idx];

        let mut source = File::open(file_path).map_err(|e| format!("ERROR: Failed to open {}: {}", file_path.display(), e))?;
        let mut buf = vec![0u8; CLUSTER_SIZE as usize];

        if let Some((partition_start, cluster_heap_offset_sectors, chain)) = offset_to_chain.get(&entry.offset) {
            let heap_byte_offset = partition_start + *cluster_heap_offset_sectors as u64 * BYTES_PER_SECTOR as u64;
            let mut remaining = entry.size;

            for &cluster_num in chain {
                if remaining == 0 {
                    break;
                }
                let cluster_offset = heap_byte_offset + (cluster_num as u64 - 2) * cluster_size;
                let to_write = remaining.min(cluster_size) as usize;

                source.read_exact(&mut buf[..to_write]).map_err(|e| format!("ERROR: Failed to read {}: {}", file_path.display(), e))?;
                rw.seek(SeekFrom::Start(cluster_offset)).map_err(|e| format!("ERROR: Failed to seek to cluster {}: {}", cluster_num, e))?;
                rw.write_all(&buf[..to_write]).map_err(|e| format!("ERROR: Failed to write cluster {}: {}", cluster_num, e))?;

                remaining -= to_write as u64;
            }
        } else {
            let offset = entry.offset * 512;
            rw.seek(SeekFrom::Start(offset)).map_err(|e| format!("ERROR: Failed to seek to offset {}: {}", offset, e))?;

            let mut remaining = entry.size;
            while remaining > 0 {
                let to_read = remaining.min(buf.len() as u64) as usize;
                source.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read {}: {}", file_path.display(), e))?;
                rw.write_all(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write file data: {}", e))?;
                remaining -= to_read as u64;
            }
        }
    }

    rw.flush().map_err(|e| format!("ERROR: Failed to flush output: {}", e))?;
    Ok(())
}
