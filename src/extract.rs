use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const HEADER_SKIP: u64 = 512;

use sha1::{Digest, Sha1};

use crate::exfat::{self, BYTES_PER_SECTOR, CLUSTER_SIZE};
use crate::hash::{self, HashEntry};
use crate::header::{self, FileSystem, Partition};
use crate::skeleton::SkeletonWriter;

pub(crate) const BLOCK_SIZE: u64 = 512;

const CHUNK_SIZE: usize = 64 * 1024;

pub fn run(path: &Path) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("ERROR: Failed to open file: {}", e))?;

    let file_size = file.metadata().map_err(|e| format!("ERROR: Failed to get file metadata: {}", e))?.len();

    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(|e| format!("ERROR: Failed to read magic: {}", e))?;
    let data_offset = if &magic == b"PSV\0" || &magic == b"VCI\0" {
        reader.seek(SeekFrom::Start(HEADER_SKIP)).map_err(|e| format!("ERROR: Failed to seek past header: {}", e))?;
        HEADER_SKIP
    } else {
        reader.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: Failed to seek to start: {}", e))?;
        0
    };
    let data_size = file_size - data_offset;

    let psv_header = header::parse(&mut reader, data_size)?;

    psv_header.print(data_size);

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_stem().ok_or("ERROR: Can't get file name")?.to_string_lossy();

    let skeleton_path = parent.join(format!("{}.skeleton.zst", name));
    let hash_path = parent.join(format!("{}.files.tsv", name));
    let extract_dir = parent.join(name.as_ref());

    let mut skeleton_writer = SkeletonWriter::new(&skeleton_path).map_err(|e| format!("ERROR: Failed to create skeleton: {}", e))?;

    skeleton_writer.write_bytes(&psv_header.raw).map_err(|e| format!("ERROR: Failed to write header to skeleton: {}", e))?;

    let mut hash_entries: Vec<HashEntry> = Vec::new();

    let mut partitions: Vec<&Partition> = psv_header.partitions.iter().collect();
    partitions.sort_by_key(|p| p.offset);

    let mut pos: u64 = BLOCK_SIZE;

    for partition in &partitions {
        let partition_start = partition.offset as u64 * BLOCK_SIZE;
        let partition_size = partition.size as u64 * BLOCK_SIZE;

        if partition_start > pos {
            process_gap(&mut reader, &mut skeleton_writer, pos, partition_start)?;
        } else if partition_start < pos {
            return Err(format!("ERROR: Partition at offset 0x{:X} overlaps with previous partition", partition_start));
        }

        match partition.filesystem {
            FileSystem::ExFat => {
                process_exfat(&mut reader, &mut skeleton_writer, partition, &mut hash_entries, &extract_dir, data_offset)?;
            }
            _ => {
                process_raw(&mut reader, &mut skeleton_writer, partition)?;
            }
        }

        pos = partition_start + partition_size;
    }

    let header_size = psv_header.image_size() as u64 * BLOCK_SIZE;

    if pos < header_size.min(data_size) {
        process_gap(&mut reader, &mut skeleton_writer, pos, header_size.min(data_size))?;
    }

    if data_size < header_size {
        skeleton_writer.write_zeros(header_size - data_size).map_err(|e| format!("ERROR: Failed to write padding zeros: {}", e))?;
    } else if data_size > header_size {
        let mut buf = vec![0u8; CHUNK_SIZE as usize];
        let mut remaining = data_size - header_size.max(pos);
        let mut has_nonzero = false;
        while remaining > 0 {
            let to_read = remaining.min(CHUNK_SIZE as u64) as usize;
            reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read trailing data: {}", e))?;
            if !has_nonzero && buf[..to_read].iter().any(|&b| b != 0) {
                has_nonzero = true;
            }
            remaining -= to_read as u64;
        }
        if has_nonzero {
            eprintln!("WARNING: File has {} bytes of non-zero data past the header image size", data_size - header_size);
        }
    }

    skeleton_writer.finish().map_err(|e| format!("ERROR: Failed to finalize skeleton: {}", e))?;

    hash::write_hash_file(&hash_path, &hash_entries).map_err(|e| format!("ERROR: Failed to write hash file: {}", e))?;

    println!("Created: {}", hash_path.display());
    println!("Created: {}", skeleton_path.display());

    Ok(())
}

fn copy_region(reader: &mut impl Read, writer: &mut SkeletonWriter, offset: u64, size: u64) -> Result<bool, String> {
    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut all_zeros = true;

    while remaining > 0 {
        let to_read = remaining.min(CHUNK_SIZE as u64) as usize;
        reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read at offset 0x{:X}: {}", offset, e))?;

        if all_zeros && !buf[..to_read].iter().all(|&b| b == 0) {
            all_zeros = false;
        }

        writer.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write at offset 0x{:X}: {}", offset, e))?;

        remaining -= to_read as u64;
    }

    Ok(all_zeros)
}

fn process_gap(reader: &mut impl Read, writer: &mut SkeletonWriter, start: u64, end: u64) -> Result<(), String> {
    let all_zeros = copy_region(reader, writer, start, end - start)?;
    if !all_zeros {
        eprintln!("WARNING: Gap region 0x{:X}-0x{:X} contains non-zero data", start, end);
    }
    Ok(())
}

fn process_raw(reader: &mut impl Read, writer: &mut SkeletonWriter, partition: &Partition) -> Result<(), String> {
    let partition_start = partition.offset as u64 * BLOCK_SIZE;
    let partition_size = partition.size as u64 * BLOCK_SIZE;
    let all_zeros = copy_region(reader, writer, partition_start, partition_size)?;
    if all_zeros {
        println!("{} partition at 0x{:X} ({} blocks): entirely zeroed", partition.filesystem, partition_start, partition.size);
    } else {
        println!("{} partition at 0x{:X} ({} blocks): contains data", partition.filesystem, partition_start, partition.size);
    }
    Ok(())
}

fn process_exfat(reader: &mut (impl Read + Seek), writer: &mut SkeletonWriter, partition: &Partition, hash_entries: &mut Vec<HashEntry>, extract_dir: &Path, data_offset: u64) -> Result<(), String> {
    let partition_start = partition.offset as u64 * BLOCK_SIZE;
    let partition_size = partition.size as u64 * BLOCK_SIZE;

    let ctx = exfat::parse_seekable(reader, data_offset + partition_start, partition_size)?;
    let cluster_to_file = exfat::build_cluster_map(&ctx.files);

    let num_files = ctx.files.len();
    let mut hashers: Vec<Sha1> = (0..num_files).map(|_| Sha1::new()).collect();
    let mut remaining_bytes: Vec<u64> = ctx.files.iter().map(|f| f.size).collect();
    let mut file_writers: Vec<Option<BufWriter<File>>> = Vec::with_capacity(num_files);
    for file_info in &ctx.files {
        let relative_path = file_info.path.trim_start_matches('/');
        let out_path = extract_dir.join(relative_path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("ERROR: Failed to create directory {}: {}", parent.display(), e))?;
        }
        let out_file = File::create(&out_path).map_err(|e| format!("ERROR: Failed to create file {}: {}", out_path.display(), e))?;
        file_writers.push(Some(BufWriter::new(out_file)));
    }

    let cluster_size = CLUSTER_SIZE as usize;
    let cluster_heap_start = ctx.cluster_heap_offset_sectors as u64 * BYTES_PER_SECTOR as u64;

    reader.seek(SeekFrom::Start(data_offset + partition_start)).map_err(|e| format!("ERROR: Failed to seek back to partition start: {}", e))?;

    let pre_heap_size = cluster_heap_start as usize;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut pre_remaining = pre_heap_size;
    while pre_remaining > 0 {
        let to_read = pre_remaining.min(CHUNK_SIZE);
        reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read exFAT pre-heap: {}", e))?;
        writer.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write exFAT pre-heap: {}", e))?;
        pre_remaining -= to_read;
    }

    let mut cluster_buf = vec![0u8; cluster_size];
    let total_clusters = ctx.cluster_count as usize;
    for cluster_idx in 0..total_clusters {
        let cluster_number = cluster_idx as u32 + 2;
        let actual_size = cluster_size.min((partition_size as usize) - pre_heap_size - cluster_idx * cluster_size);
        reader.read_exact(&mut cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to read cluster {}: {}", cluster_number, e))?;

        if ctx.metadata_clusters.contains(&cluster_number) {
            writer.write_bytes(&cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to write metadata cluster: {}", e))?;
        } else if let Some(&file_idx) = cluster_to_file.get(&cluster_number) {
            let remaining = remaining_bytes[file_idx];
            if remaining > 0 {
                let bytes_to_hash = remaining.min(actual_size as u64) as usize;
                hashers[file_idx].update(&cluster_buf[..bytes_to_hash]);
                if let Some(ref mut w) = file_writers[file_idx] {
                    w.write_all(&cluster_buf[..bytes_to_hash]).map_err(|e| format!("ERROR: Failed to write file data: {}", e))?;
                }
                remaining_bytes[file_idx] -= bytes_to_hash as u64;
                if remaining_bytes[file_idx] == 0 {
                    if let Some(w) = file_writers[file_idx].take() {
                        w.into_inner().map_err(|e| format!("ERROR: Failed to flush file: {}", e))?;
                    }
                }
            }
            writer.write_zeros(actual_size as u64).map_err(|e| format!("ERROR: Failed to write zeros for file cluster: {}", e))?;
        } else {
            writer.write_bytes(&cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to write free cluster: {}", e))?;
        }
    }

    let cluster_heap_end = pre_heap_size + total_clusters * cluster_size;
    if cluster_heap_end < partition_size as usize {
        let trailing = partition_size as usize - cluster_heap_end;
        let mut trail_remaining = trailing;
        while trail_remaining > 0 {
            let to_read = trail_remaining.min(CHUNK_SIZE);
            reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read trailing partition data: {}", e))?;
            writer.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write trailing partition data: {}", e))?;
            trail_remaining -= to_read;
        }
    }

    for (file_idx, file_info) in ctx.files.iter().enumerate() {
        if let Some(w) = file_writers[file_idx].take() {
            w.into_inner().map_err(|e| format!("ERROR: Failed to flush file: {}", e))?;
        }

        let hasher = std::mem::replace(&mut hashers[file_idx], Sha1::new());
        let hash_result = hasher.finalize();
        let sha1 = {
            let mut hex = String::with_capacity(40);
            for byte in hash_result.iter() {
                use std::fmt::Write as FmtWrite;
                write!(hex, "{:02x}", byte).unwrap();
            }
            hex
        };

        let partition_offset = if let Some(&first_cluster) = file_info.chain.first() { cluster_heap_start + (first_cluster as u64 - 2) * cluster_size as u64 } else { 0 };

        let offset = (partition_start + partition_offset) / BLOCK_SIZE;
        hash_entries.push(HashEntry { sha1, offset, size: file_info.size, path: file_info.path.clone() });
    }

    println!("exFAT partition at 0x{:X}: {} files extracted", partition_start, ctx.files.len());

    Ok(())
}
