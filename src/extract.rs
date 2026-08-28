use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::common::{self, BLOCK_SIZE};
use crate::exfat::{self, BYTES_PER_SECTOR, CLUSTER_SIZE};
use crate::hash::{self, HashEntry};
use crate::header::{self, FileSystem, Partition};
use crate::skeleton::SkeletonWriter;

use crate::common::BLACKFIN_MAGIC;
use crate::common::{BLACKFIN_OFFSET, BLACKFIN_SIZE, HEADER_SKIP, LIC1_OFFSET, LIC1_SIZE, LIC2_OFFSET, LIC2_SIZE, UNK_OFFSET, UNK_SIZE};

struct FileData {
    sfo: Option<Vec<u8>>,
    pup: Option<Vec<u8>>,
}

const CHUNK_SIZE: usize = 64 * 1024;

pub fn analyze(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return common::iter_image_files(path, |p| run(p, false));
    }
    run(path, false)
}

pub fn extract(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return common::iter_image_files(path, |p| run(p, true));
    }
    run(path, true)
}

fn run(path: &Path, extract: bool) -> Result<(), String> {
    let mut file_data = FileData { sfo: None, pup: None };
    let file = File::open(path).map_err(|e| format!("ERROR: Failed to open file: {}", e))?;

    if extract {
        println!("Extracting {}", path.display());
    } else {
        println!("Analyzing {}", path.display());
    }
    let file_size = file.metadata().map_err(|e| format!("ERROR: Failed to get file metadata: {}", e))?.len();
    common::check_file_size(file_size, path)?;

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
    let psv_header = header::parse(&mut reader, file_size - data_offset)?;

    psv_header.print(file_size - data_offset, extract);

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_stem().ok_or("ERROR: Can't get file name")?.to_string_lossy();
    let basename = parent.join(name.as_ref());

    if extract {
        if data_offset > 0 {
            reader.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: {}", e))?;
            let mut hdr = vec![0u8; data_offset as usize];
            reader.read_exact(&mut hdr).map_err(|e| format!("ERROR: Failed to read wrapper header: {}", e))?;
            common::save_hdr(&hdr, &basename)?;
            reader.seek(SeekFrom::Start(data_offset + BLOCK_SIZE)).map_err(|e| format!("ERROR: {}", e))?;
        }

        let skeleton_path = parent.join(format!("{}.skeleton.zst", name));
        let hash_path = parent.join(format!("{}.tsv", name));
        let extract_dir = parent.join(name.as_ref());

        if skeleton_path.exists() {
            return Err(format!("ERROR: {} already exists", skeleton_path.display()));
        }
        if hash_path.exists() {
            return Err(format!("ERROR: {} already exists", hash_path.display()));
        }
        if extract_dir.exists() {
            return Err(format!("ERROR: {} already exists", extract_dir.display()));
        }
    }

    let skeleton_path = parent.join(format!("{}.skeleton.zst", name));
    let hash_path = parent.join(format!("{}.tsv", name));
    let extract_dir = parent.join(name.as_ref());

    let mut skeleton_writer = if extract { Some(SkeletonWriter::new(&skeleton_path).map_err(|e| format!("ERROR: Failed to create skeleton: {}", e))?) } else { None };

    if let Some(ref mut sw) = skeleton_writer {
        sw.write_bytes(&psv_header.raw).map_err(|e| format!("ERROR: Failed to write header to skeleton: {}", e))?;
    }

    let mut hash_entries: Vec<HashEntry> = Vec::new();

    let mut partitions: Vec<&Partition> = psv_header.partitions.iter().collect();
    partitions.sort_by_key(|p| p.offset);
    let sorted_partitions: Vec<Partition> = partitions.iter().map(|p| (*p).clone()).collect();
    let part_names = header::partition_names(&sorted_partitions, |fs| matches!(fs, FileSystem::ExFat));
    let multi_partition = part_names.iter().filter(|n| n.is_some()).count() > 1;

    let mut pos: u64 = data_offset + BLOCK_SIZE;

    for (partition, part_name) in partitions.iter().zip(part_names.iter()) {
        let partition_start = data_offset + partition.offset as u64 * BLOCK_SIZE;
        let partition_size = partition.size as u64 * BLOCK_SIZE;

        if partition_start > pos {
            process_gap(&mut reader, &mut skeleton_writer, pos, partition_start, data_offset, &basename)?;
        } else if partition_start < pos {
            return Err(format!("ERROR: Partition at offset 0x{:X} overlaps with previous partition", partition_start));
        }

        match partition.filesystem {
            FileSystem::ExFat => {
                let partition_dir = match part_name {
                    Some(name) if multi_partition => extract_dir.join(name),
                    _ => extract_dir.clone(),
                };
                let num_files = process_exfat(&mut reader, &mut skeleton_writer, partition, &mut hash_entries, &partition_dir, data_offset, &basename, &mut file_data)?;
                let display_name = partition.code.name().unwrap_or("unknown");
                if extract {
                    println!("{}: {} files extracted to {}", display_name, num_files, partition_dir.display());
                } else {
                    println!("{}: {} files", display_name, num_files);
                }
            }
            _ => {
                let img_path = part_name.as_ref().map(|name| extract_dir.join(format!("{}.img", name)));
                process_non_exfat(&mut reader, &mut skeleton_writer, partition, img_path.as_deref())?;
            }
        }

        pos = partition_start + partition_size;
    }

    let img_end = data_offset + psv_header.image_size() as u64 * BLOCK_SIZE;

    if pos < img_end.min(file_size) {
        process_gap(&mut reader, &mut skeleton_writer, pos, img_end.min(file_size), data_offset, &basename)?;
    }

    if file_size < img_end {
        if let Some(ref mut sw) = skeleton_writer {
            sw.write_zeros(img_end - file_size).map_err(|e| format!("ERROR: Failed to write padding zeros: {}", e))?;
        }
    } else if file_size > img_end {
        let mut buf = vec![0u8; CHUNK_SIZE as usize];
        let mut remaining = file_size - img_end.max(pos);
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
            println!("WARNING: File has {} bytes of non-zero data past the header image size", file_size - img_end);
        }
        // TODO: Solve trimming
        if let Some(ref mut sw) = skeleton_writer {
            sw.write_zeros(file_size - img_end.max(pos)).map_err(|e| format!("ERROR: Failed to write trailing zeros to skeleton: {}", e))?;
        }
    }

    if let Some(sw) = skeleton_writer {
        sw.finish().map_err(|e| format!("ERROR: Failed to finalize skeleton: {}", e))?;
        hash::write_hash_file(&hash_path, &hash_entries).map_err(|e| format!("ERROR: Failed to write hash file: {}", e))?;
        println!("File hashes saved to: {}", hash_path.display());
        println!("Image skeleton saved to: {}", skeleton_path.display());
    }

    validate_game(&file_data, &hash_entries);

    Ok(())
}

fn copy_region(reader: &mut impl Read, writer: &mut Option<SkeletonWriter>, offset: u64, size: u64) -> Result<bool, String> {
    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut all_zeros = true;

    while remaining > 0 {
        let to_read = remaining.min(CHUNK_SIZE as u64) as usize;
        reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read at offset 0x{:X}: {}", offset, e))?;

        if all_zeros && !buf[..to_read].iter().all(|&b| b == 0) {
            all_zeros = false;
        }

        if let Some(sw) = writer {
            sw.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write at offset 0x{:X}: {}", offset, e))?;
        }

        remaining -= to_read as u64;
    }

    Ok(all_zeros)
}

fn process_gap(reader: &mut impl Read, writer: &mut Option<SkeletonWriter>, start: u64, end: u64, data_offset: u64, basename: &Path) -> Result<(), String> {
    let special: &[(u64, u64)] = &[(data_offset + UNK_OFFSET, data_offset + UNK_OFFSET + UNK_SIZE), (data_offset + BLACKFIN_OFFSET, data_offset + BLACKFIN_OFFSET + BLACKFIN_SIZE)];

    let mut pos = start;

    for &(region_start, region_end) in special {
        if region_start >= end || region_end <= start {
            continue;
        }
        let region_start = region_start.max(start);
        let region_end = region_end.min(end);

        if region_start > pos {
            let all_zeros = copy_region(reader, writer, pos, region_start - pos)?;
            if !all_zeros {
                println!("WARNING: Gap region 0x{:X}-0x{:X} contains non-zero data", pos, region_start);
            }
        }

        let len = region_end - region_start;
        let mut buf = vec![0u8; len as usize];
        reader.read_exact(&mut buf).map_err(|e| format!("ERROR: Failed to read region at 0x{:X}: {}", region_start, e))?;

        if region_start == data_offset + UNK_OFFSET {
            if buf.iter().any(|&b| b != 0) {
                if writer.is_some() {
                    common::save_unk(&buf, basename)?;
                    println!("Wiped unique header data from skeleton");
                }
            }
        } else if region_start == data_offset + BLACKFIN_OFFSET {
            if buf.len() >= BLACKFIN_MAGIC.len() && &buf[..BLACKFIN_MAGIC.len()] == BLACKFIN_MAGIC {
                if writer.is_some() {
                    common::save_blackfin(&buf, basename)?;
                    println!("Wiped BlackFin data from skeleton");
                }
            }
        }

        if let Some(sw) = writer {
            sw.write_zeros(len).map_err(|e| format!("ERROR: Failed to write zeros for region at 0x{:X}: {}", region_start, e))?;
        }
        pos = region_end;
    }

    if pos < end {
        let all_zeros = copy_region(reader, writer, pos, end - pos)?;
        if !all_zeros {
            println!("WARNING: Gap region 0x{:X}-0x{:X} contains non-zero data", pos, end);
        }
    }

    Ok(())
}

fn process_non_exfat(reader: &mut impl Read, writer: &mut Option<SkeletonWriter>, partition: &Partition, img_path: Option<&Path>) -> Result<(), String> {
    let partition_start = partition.offset as u64 * BLOCK_SIZE;
    let partition_size = partition.size as u64 * BLOCK_SIZE;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut remaining = partition_size;
    let mut data: Vec<u8> = if img_path.is_some() { Vec::with_capacity(partition_size as usize) } else { Vec::new() };
    let mut all_zeros = true;
    while remaining > 0 {
        let to_read = remaining.min(CHUNK_SIZE as u64) as usize;
        reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read {} partition at 0x{:X}: {}", partition.filesystem, partition_start, e))?;
        if all_zeros && buf[..to_read].iter().any(|&b| b != 0) {
            all_zeros = false;
        }
        if img_path.is_some() {
            data.extend_from_slice(&buf[..to_read]);
        }
        remaining -= to_read as u64;
    }
    if let Some(sw) = writer {
        sw.write_zeros(partition_size).map_err(|e| format!("ERROR: Failed to write zeros for {} partition: {}", partition.filesystem, e))?;
    }
    if !all_zeros && writer.is_some() {
        if let Some(path) = img_path {
            std::fs::write(path, &data).map_err(|e| format!("ERROR: Failed to write {}: {}", path.display(), e))?;
            println!("{} partition at 0x{:X}: extracted to {}", partition.filesystem, partition_start, path.display());
        }
    }
    Ok(())
}

fn process_exfat(
    reader: &mut (impl Read + Seek),
    writer: &mut Option<SkeletonWriter>,
    partition: &Partition,
    hash_entries: &mut Vec<HashEntry>,
    extract_dir: &Path,
    data_offset: u64,
    basename: &Path,
    file_data: &mut FileData,
) -> Result<usize, String> {
    let partition_start = partition.offset as u64 * BLOCK_SIZE;
    let partition_size = partition.size as u64 * BLOCK_SIZE;

    let ctx = exfat::parse_seekable(reader, data_offset + partition_start, partition_size)?;
    let cluster_to_file = exfat::build_cluster_map(&ctx.files);

    let num_files = ctx.files.len();
    let mut hashers: Vec<Sha256> = (0..num_files).map(|_| Sha256::new()).collect();
    let mut remaining_bytes: Vec<u64> = ctx.files.iter().map(|f| f.size).collect();
    let mut file_writers: Vec<Option<BufWriter<File>>> = Vec::with_capacity(num_files);

    let is_license_rif: Vec<bool> = ctx
        .files
        .iter()
        .map(|f| {
            let parts: Vec<&str> = f.path.trim_start_matches('/').split('/').collect();
            parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app") && parts[3].ends_with(".rif")
        })
        .collect();

    let sfo_idx = ctx.files.iter().position(|f| f.path.eq_ignore_ascii_case("/gc/param.sfo"));
    let pup_idx = ctx.files.iter().position(|f| f.path.eq_ignore_ascii_case("/psp2/update/psp2updat.pup"));
    let mut sfo_buf: Vec<u8> = Vec::new();
    let mut pup_buf: Vec<u8> = Vec::new();

    if writer.is_some() {
        for file_info in &ctx.files {
            let out_path = extract_dir.join(file_info.path.trim_start_matches('/'));
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("ERROR: Failed to create directory {}: {}", parent.display(), e))?;
            }
            let out_file = File::create(&out_path).map_err(|e| format!("ERROR: Failed to create file {}: {}", out_path.display(), e))?;
            file_writers.push(Some(BufWriter::new(out_file)));
        }
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
        if let Some(sw) = writer {
            sw.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write exFAT pre-heap: {}", e))?;
        }
        pre_remaining -= to_read;
    }

    let mut cluster_buf = vec![0u8; cluster_size];
    let total_clusters = ctx.cluster_count as usize;
    for cluster_idx in 0..total_clusters {
        let cluster_number = cluster_idx as u32 + 2;
        let actual_size = cluster_size.min((partition_size as usize) - pre_heap_size - cluster_idx * cluster_size);
        reader.read_exact(&mut cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to read cluster {}: {}", cluster_number, e))?;

        if ctx.metadata_clusters.contains(&cluster_number) {
            if let Some(sw) = writer {
                sw.write_bytes(&cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to write metadata cluster: {}", e))?;
            }
        } else if let Some(&file_idx) = cluster_to_file.get(&cluster_number) {
            let remaining = remaining_bytes[file_idx];
            if remaining > 0 {
                let bytes_to_hash = remaining.min(actual_size as u64) as usize;
                let file_offset = ctx.files[file_idx].size - remaining;

                if sfo_idx == Some(file_idx) {
                    sfo_buf.extend_from_slice(&cluster_buf[..bytes_to_hash]);
                    if remaining_bytes[file_idx] - bytes_to_hash as u64 == 0 {
                        file_data.sfo = Some(std::mem::take(&mut sfo_buf));
                    }
                }
                if pup_idx == Some(file_idx) {
                    pup_buf.extend_from_slice(&cluster_buf[..bytes_to_hash]);
                    if remaining_bytes[file_idx] - bytes_to_hash as u64 == 0 {
                        file_data.pup = Some(std::mem::take(&mut pup_buf));
                    }
                }

                if is_license_rif[file_idx] && writer.is_some() {
                    if file_offset == 0 {
                        let verbatim = cluster_buf[..bytes_to_hash.min(ctx.files[file_idx].size as usize)].to_vec();
                        common::save_rif(&verbatim, basename)?;
                    }
                    if wipe_license_data(&mut cluster_buf, file_offset, bytes_to_hash) {
                        println!("Wiped unique license data from {}", ctx.files[file_idx].path);
                    }
                }

                hashers[file_idx].update(&cluster_buf[..bytes_to_hash]);
                if let Some(ref mut w) = file_writers.get_mut(file_idx).and_then(|w| w.as_mut()) {
                    w.write_all(&cluster_buf[..bytes_to_hash]).map_err(|e| format!("ERROR: Failed to write file data: {}", e))?;
                }
                remaining_bytes[file_idx] -= bytes_to_hash as u64;
                if remaining_bytes[file_idx] == 0 {
                    if let Some(w) = file_writers.get_mut(file_idx).and_then(|w| w.take()) {
                        w.into_inner().map_err(|e| format!("ERROR: Failed to flush file: {}", e))?;
                    }
                }
            }
            if let Some(sw) = writer {
                sw.write_zeros(actual_size as u64).map_err(|e| format!("ERROR: Failed to write zeros for file cluster: {}", e))?;
            }
        } else {
            if let Some(sw) = writer {
                sw.write_bytes(&cluster_buf[..actual_size]).map_err(|e| format!("ERROR: Failed to write free cluster: {}", e))?;
            }
        }
    }

    let cluster_heap_end = pre_heap_size + total_clusters * cluster_size;
    if cluster_heap_end < partition_size as usize {
        let trailing = partition_size as usize - cluster_heap_end;
        let mut trail_remaining = trailing;
        while trail_remaining > 0 {
            let to_read = trail_remaining.min(CHUNK_SIZE);
            reader.read_exact(&mut buf[..to_read]).map_err(|e| format!("ERROR: Failed to read trailing partition data: {}", e))?;
            if let Some(sw) = writer {
                sw.write_bytes(&buf[..to_read]).map_err(|e| format!("ERROR: Failed to write trailing partition data: {}", e))?;
            }
            trail_remaining -= to_read;
        }
    }

    if writer.is_some() {
        for (file_idx, _) in ctx.files.iter().enumerate() {
            if let Some(w) = file_writers[file_idx].take() {
                w.into_inner().map_err(|e| format!("ERROR: Failed to flush file: {}", e))?;
            }
        }
    }

    for (file_idx, file_info) in ctx.files.iter().enumerate() {
        let hasher = std::mem::replace(&mut hashers[file_idx], Sha256::new());
        let hash_result = hasher.finalize();
        let sha256 = {
            let mut hex = String::with_capacity(64);
            for byte in hash_result.iter() {
                use std::fmt::Write as FmtWrite;
                write!(hex, "{:02x}", byte).unwrap();
            }
            hex
        };

        let partition_offset = if let Some(&first_cluster) = file_info.chain.first() { cluster_heap_start + (first_cluster as u64 - 2) * cluster_size as u64 } else { 0 };

        let offset = (partition_start + partition_offset) / BLOCK_SIZE;
        hash_entries.push(HashEntry { sha256, offset, size: file_info.size, path: file_info.path.clone() });
    }

    Ok(ctx.files.len())
}

fn parse_psf(data: Vec<u8>) -> std::collections::HashMap<String, Vec<u8>> {
    let mut map = std::collections::HashMap::new();
    if data.len() < 20 || &data[0..4] != b"\0PSF" {
        return map;
    }
    let key_table = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let data_table = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    let entry_count = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
    for i in 0..entry_count {
        let e = 20 + i * 16;
        if e + 16 > data.len() {
            break;
        }
        let key_offset = u16::from_le_bytes(data[e..e + 2].try_into().unwrap()) as usize;
        let data_len = u32::from_le_bytes(data[e + 4..e + 8].try_into().unwrap()) as usize;
        let data_offset = u32::from_le_bytes(data[e + 12..e + 16].try_into().unwrap()) as usize;
        let key_start = key_table + key_offset;
        let key_end = match data[key_start..].iter().position(|&b| b == 0) {
            Some(p) => key_start + p,
            None => continue,
        };
        let val_start = data_table + data_offset;
        if val_start + data_len > data.len() {
            continue;
        }
        let key = String::from_utf8_lossy(&data[key_start..key_end]).to_string();
        map.insert(key, data[val_start..val_start + data_len].to_vec());
    }
    map
}

fn bytes_at(data: &[u8], offset: usize, len: usize) -> Option<Vec<u8>> {
    if offset + len > data.len() {
        return None;
    }
    Some(data[offset..offset + len].to_vec())
}

fn version_at(data: &[u8], offset: usize) -> Option<[u8; 4]> {
    let b = bytes_at(data, offset, 4)?;
    Some([b[0], b[1], b[2], b[3]])
}

fn format_version(b: [u8; 4]) -> String {
    format!("{}.{:02X}", b[3], b[2])
}

fn parse_sfo_psf(sfo: &[u8]) -> Option<Vec<u8>> {
    if sfo.len() < 0x604 {
        return None;
    }
    let buf = &sfo[0x600..];
    if &buf[0..4] != b"\0PSF" {
        return None;
    }
    Some(buf[..buf.len().min(0x1000)].to_vec())
}

fn psf_str(val: &[u8]) -> &str {
    std::str::from_utf8(val).unwrap_or("").trim_end_matches('\0')
}

fn psf_ver_bytes(val: &[u8]) -> Option<[u8; 4]> {
    if val.len() >= 4 { Some([val[0], val[1], val[2], val[3]]) } else { None }
}

const KNOWN_PSF_KEYS: &[&str] = &["TITLE_ID", "CATEGORY", "VERSION", "PSP2_SYSTEM_VER", "PSP2_SYSTEM_ROOT_VER", "PSP2_DISP_VER", "SYSPRM"];

fn validate_game(file_data: &FileData, hash_entries: &[HashEntry]) {
    let psf = file_data.sfo.as_deref().and_then(parse_sfo_psf).map(parse_psf);

    let title_id = file_data.sfo.as_deref().and_then(|s| bytes_at(s, 0x410, 9)).map(|b| String::from_utf8_lossy(&b).trim_end_matches('\0').to_string());

    if let Some(ref id) = title_id {
        if let Some(ref p) = psf {
            if let Some(val) = p.get("TITLE_ID") {
                if !psf_str(val).eq_ignore_ascii_case(id) {
                    println!("WARNING: Title ID mismatch between SFO header (0x410: {}) and PSF TITLE_ID ({})", id, psf_str(val));
                }
            } else {
                println!("WARNING: TITLE_ID not found in PSF at 0x600");
            }
        }
        let app_dir = hash_entries.iter().find_map(|e| {
            let parts: Vec<&str> = e.path.trim_start_matches('/').splitn(3, '/').collect();
            if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("app") { Some(parts[1].to_string()) } else { None }
        });
        let license_dir = hash_entries.iter().find_map(|e| {
            let parts: Vec<&str> = e.path.trim_start_matches('/').split('/').collect();
            if parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app") { Some(parts[2].to_string()) } else { None }
        });
        if let Some(ref app) = app_dir {
            if !app.eq_ignore_ascii_case(id) {
                println!("WARNING: Title ID mismatch: SFO says {}, /app/ folder is {}", id, app);
            }
        }
        if let Some(ref lic) = license_dir {
            if !lic.eq_ignore_ascii_case(id) {
                println!("WARNING: Title ID mismatch: SFO says {}, /license/app/ folder is {}", id, lic);
            }
        }
        println!("Title ID: {}", id);
    }

    if let Some(ref p) = psf {
        if let Some(val) = p.get("CATEGORY") {
            if !psf_str(val).eq_ignore_ascii_case("gc") {
                println!("WARNING: Unexpected CATEGORY: {}", psf_str(val));
            }
        } else {
            println!("WARNING: CATEGORY not found in PSF at 0x600");
        }
        if let Some(val) = p.get("VERSION") {
            println!("Game version: {}", psf_str(val));
        } else {
            println!("WARNING: VERSION not found in PSF at 0x600");
        }
        for (key, val) in p.iter().filter(|(k, _)| !KNOWN_PSF_KEYS.contains(&k.as_str())) {
            println!("WARNING: Unexpected PSF key: {}={}", key, psf_str(val));
        }
        if let Some(val) = p.get("SYSPRM") {
            if !val.is_empty() {
                println!("WARNING: SYSPRM is non-empty: {:02X?}", val);
            }
        }
    }

    let sfo_ver = file_data.sfo.as_deref().and_then(|s| version_at(s, 0x40C));
    let psf_ver = psf.as_ref().and_then(|p| p.get("PSP2_SYSTEM_VER").and_then(|v| psf_ver_bytes(v)));
    let psf_root_ver = psf.as_ref().and_then(|p| p.get("PSP2_SYSTEM_ROOT_VER").and_then(|v| psf_ver_bytes(v)));
    let psf_disp_ver = psf.as_ref().and_then(|p| p.get("PSP2_DISP_VER").map(|v| psf_str(v).to_string()));
    if psf.is_some() {
        if psf_ver.is_none() {
            println!("WARNING: PSP2_SYSTEM_VER not found in PSF at 0x600");
        }
        if psf_root_ver.is_none() {
            println!("WARNING: PSP2_SYSTEM_ROOT_VER not found in PSF at 0x600");
        }
        if psf_disp_ver.is_none() {
            println!("WARNING: PSP2_DISP_VER not found in PSF at 0x600");
        }
    }
    if let (Some(s), Some(p)) = (sfo_ver, psf_ver) {
        if s != p {
            println!("WARNING: System Update version mismatch between SFO header (0x40C: {}) and PSP2_SYSTEM_VER ({})", format_version(s), format_version(p));
        }
    }
    if let (Some(s), Some(p)) = (sfo_ver, psf_root_ver) {
        if s != p && p != [0u8; 4] {
            println!("WARNING: System Update version mismatch between SFO header (0x40C: {}) and PSP2_SYSTEM_ROOT_VER ({})", format_version(s), format_version(p));
        }
    }
    if let (Some(s), Some(ref d)) = (sfo_ver, psf_disp_ver) {
        let expected = format!("{:02}.{:02X}0", s[3], s[2]);
        if d.as_str() != expected {
            println!("WARNING: System Update version mismatch between SFO header (0x40C: {}) and PSP2_DISP_VER ({})", format_version(s), d);
        }
    }
    let pup_ver = file_data.pup.as_deref().and_then(|p| version_at(p, 0x10));
    match (sfo_ver, pup_ver) {
        (Some(s), Some(p)) if s == p => println!("System Update version: {}", format_version(s)),
        (Some(s), Some(p)) => {
            println!("WARNING: System Update version mismatch between SFO and PUP");
            println!("System Update version (SFO): {}", format_version(s));
            println!("System Update version (PUP): {}", format_version(p));
        }
        (Some(s), None) => {
            println!("WARNING: psp2updat.pup missing, cannot verify System Update version");
            println!("System Update version (SFO): {}", format_version(s));
        }
        (None, Some(p)) => {
            println!("WARNING: param.sfo missing, cannot verify System Update version");
            println!("System Update version (PUP): {}", format_version(p));
        }
        (None, None) => println!("WARNING: param.sfo and psp2updat.pup missing, cannot determine System Update version"),
    }

    let app_files: Vec<&str> = hash_entries
        .iter()
        .filter(|e| {
            let parts: Vec<&str> = e.path.trim_start_matches('/').split('/').collect();
            parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app")
        })
        .map(|e| e.path.as_str())
        .collect();
    let rif_count = app_files.iter().filter(|p| p.ends_with(".rif")).count();
    if rif_count == 0 {
        println!("WARNING: No .rif file found in /license/app/");
    } else {
        for entry in hash_entries.iter().filter(|e| {
            let parts: Vec<&str> = e.path.trim_start_matches('/').split('/').collect();
            parts.len() == 4 && parts[0].eq_ignore_ascii_case("license") && parts[1].eq_ignore_ascii_case("app") && parts[3].ends_with(".rif")
        }) {
            if entry.size != 0x200 {
                println!("WARNING: {} is {} bytes, expected 0x200", entry.path, entry.size);
            }
            let filename = entry.path.trim_start_matches('/').split('/').nth(3).unwrap_or("");
            if filename.len().saturating_sub(4) != 32 {
                println!("WARNING: {} filename stem is not 32 characters", entry.path);
            }
        }
        let rif_dirs: std::collections::HashSet<&str> = app_files.iter().map(|p| p.trim_start_matches('/').splitn(4, '/').nth(2).unwrap_or("")).collect();
        if rif_dirs.len() > 1 {
            println!("WARNING: Multiple folders found in /license/app/: {:?}", rif_dirs);
        }
        if app_files.len() > 1 {
            println!("WARNING: Multiple files found in /license/app/: {} files", app_files.len());
        }
    }

    let app_dirs: std::collections::HashSet<&str> = hash_entries
        .iter()
        .filter_map(|e| {
            let parts: Vec<&str> = e.path.trim_start_matches('/').splitn(3, '/').collect();
            if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("app") { Some(parts[1]) } else { None }
        })
        .collect();
    if app_dirs.is_empty() {
        println!("WARNING: No /app/ folder found");
    } else if app_dirs.len() > 1 {
        println!("WARNING: Multiple subfolders found in /app/: {:?}", app_dirs);
    } else {
        let dir = app_dirs.iter().next().unwrap();
        if dir.len() != 9 {
            println!("WARNING: /app/{} subfolder name is not 9 characters (got {})", dir, dir.len());
        }
        if !hash_entries.iter().any(|e| e.path.eq_ignore_ascii_case(&format!("/app/{}/eboot.bin", dir))) {
            println!("WARNING: No eboot.bin found in /app/{}/", dir);
        }
    }
}

fn wipe_license_data(buf: &mut [u8], file_offset: u64, chunk_len: usize) -> bool {
    let a = zero_range(buf, file_offset, chunk_len, LIC1_OFFSET, LIC1_OFFSET + LIC1_SIZE);
    let b = zero_range(buf, file_offset, chunk_len, LIC2_OFFSET, LIC2_OFFSET + LIC2_SIZE);
    a || b
}

fn zero_range(buf: &mut [u8], file_offset: u64, chunk_len: usize, range_start: u64, range_end: u64) -> bool {
    let chunk_end = file_offset + chunk_len as u64;
    if range_end <= file_offset || range_start >= chunk_end {
        return false;
    }
    let start = range_start.saturating_sub(file_offset) as usize;
    let end = (range_end - file_offset).min(chunk_len as u64) as usize;
    let had_data = buf[start..end].iter().any(|&b| b != 0);
    buf[start..end].fill(0);
    had_data
}
