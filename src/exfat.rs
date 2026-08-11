use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

pub const BYTES_PER_SECTOR: u32 = 512;
pub const SECTORS_PER_CLUSTER: u32 = 64;
pub const CLUSTER_SIZE: u32 = BYTES_PER_SECTOR * SECTORS_PER_CLUSTER;

pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub chain: Vec<u32>,
}

pub struct ParseResult {
    pub cluster_heap_offset_sectors: u32,
    pub cluster_count: u32,
    pub files: Vec<FileInfo>,
    pub metadata_clusters: HashSet<u32>,
}

pub fn parse_seekable(reader: &mut (impl Read + Seek), base_offset: u64, partition_size: u64) -> Result<ParseResult, String> {
    reader.seek(SeekFrom::Start(base_offset)).map_err(|e| format!("ERROR: Failed to seek to exFAT partition: {}", e))?;
    let mut boot_sector = [0u8; 512];
    reader.read_exact(&mut boot_sector).map_err(|e| format!("ERROR: Failed to read exFAT boot sector: {}", e))?;

    if &boot_sector[0x03..0x0B] != b"EXFAT   " {
        return Err(format!("ERROR: Invalid exFAT signature: {:?}", String::from_utf8_lossy(&boot_sector[0x03..0x0B])));
    }

    let fat_offset = u32::from_le_bytes(boot_sector[0x50..0x54].try_into().unwrap());
    let fat_length = u32::from_le_bytes(boot_sector[0x54..0x58].try_into().unwrap());
    let cluster_heap_offset_sectors = u32::from_le_bytes(boot_sector[0x58..0x5C].try_into().unwrap());
    let cluster_count = u32::from_le_bytes(boot_sector[0x5C..0x60].try_into().unwrap());
    let root_dir_first_cluster = u32::from_le_bytes(boot_sector[0x60..0x64].try_into().unwrap());
    let bytes_per_sector = 1u32 << boot_sector[0x6C];
    let sectors_per_cluster = 1u32 << boot_sector[0x6D];

    if bytes_per_sector != BYTES_PER_SECTOR {
        return Err(format!("ERROR: Unsupported bytes per sector: {} (expected {})", bytes_per_sector, BYTES_PER_SECTOR));
    }
    if sectors_per_cluster != SECTORS_PER_CLUSTER {
        return Err(format!("ERROR: Unsupported sectors per cluster: {} (expected {})", sectors_per_cluster, SECTORS_PER_CLUSTER));
    }

    let fat_start = fat_offset as u64 * BYTES_PER_SECTOR as u64;
    let fat_size = fat_length as u64 * BYTES_PER_SECTOR as u64;
    if fat_start + fat_size > partition_size {
        return Err(format!("ERROR: FAT region (offset {} + size {}) exceeds partition size {}", fat_start, fat_size, partition_size));
    }

    reader.seek(SeekFrom::Start(base_offset + fat_start)).map_err(|e| format!("ERROR: Failed to seek to FAT: {}", e))?;
    let entry_count = (fat_size as usize / 4).min(cluster_count as usize + 2);
    let mut fat_raw = vec![0u8; entry_count * 4];
    reader.read_exact(&mut fat_raw).map_err(|e| format!("ERROR: Failed to read FAT: {}", e))?;

    let mut fat = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let offset = i * 4;
        fat.push(u32::from_le_bytes([fat_raw[offset], fat_raw[offset + 1], fat_raw[offset + 2], fat_raw[offset + 3]]));
    }

    let mut files = Vec::new();
    let mut metadata_clusters = HashSet::new();

    traverse_seekable(reader, &fat, cluster_count, cluster_heap_offset_sectors, root_dir_first_cluster, "/", &mut files, &mut metadata_clusters, base_offset)?;

    Ok(ParseResult { cluster_heap_offset_sectors, cluster_count, files, metadata_clusters })
}

pub fn build_cluster_map(files: &[FileInfo]) -> HashMap<u32, usize> {
    let mut map = HashMap::new();
    for (file_idx, file_info) in files.iter().enumerate() {
        for &cluster in &file_info.chain {
            map.insert(cluster, file_idx);
        }
    }
    map
}

fn get_chain(fat: &[u32], cluster_count: u32, start_cluster: u32) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut cluster = start_cluster;
    for _ in 0..cluster_count {
        if cluster as usize >= fat.len() {
            break;
        }
        chain.push(cluster);
        let next = fat[cluster as usize];
        if next >= 0xFFFFFFF8 || next == 0 || next as usize >= fat.len() {
            break;
        }
        cluster = next;
    }
    chain
}

fn read_cluster(reader: &mut (impl Read + Seek), base_offset: u64, cluster_heap_offset_sectors: u32, cluster_number: u32) -> Result<Vec<u8>, String> {
    let cluster_heap_start = cluster_heap_offset_sectors as u64 * BYTES_PER_SECTOR as u64;
    let offset = base_offset + cluster_heap_start + (cluster_number as u64 - 2) * CLUSTER_SIZE as u64;
    reader.seek(SeekFrom::Start(offset)).map_err(|e| format!("ERROR: Failed to seek to cluster {}: {}", cluster_number, e))?;
    let mut buf = vec![0u8; CLUSTER_SIZE as usize];
    reader.read_exact(&mut buf).map_err(|e| format!("ERROR: Failed to read cluster {}: {}", cluster_number, e))?;
    Ok(buf)
}

fn traverse_seekable(
    reader: &mut (impl Read + Seek),
    fat: &[u32],
    cluster_count: u32,
    cluster_heap_offset_sectors: u32,
    start_cluster: u32,
    path_prefix: &str,
    files: &mut Vec<FileInfo>,
    metadata_clusters: &mut HashSet<u32>,
    base_offset: u64,
) -> Result<(), String> {
    let dir_chain = get_chain(fat, cluster_count, start_cluster);
    for &c in &dir_chain {
        metadata_clusters.insert(c);
    }

    let mut dir_data = Vec::with_capacity(dir_chain.len() * CLUSTER_SIZE as usize);
    for &cluster in &dir_chain {
        let data = read_cluster(reader, base_offset, cluster_heap_offset_sectors, cluster)?;
        dir_data.extend_from_slice(&data);
    }

    let entry_count = dir_data.len() / 32;
    let mut i = 0;

    while i < entry_count {
        let offset = i * 32;
        let entry_type = dir_data[offset];

        if entry_type == 0x00 {
            break;
        }
        if entry_type & 0x80 == 0 {
            i += 1;
            continue;
        }

        match entry_type {
            0x85 => {
                let secondary_count = dir_data[offset + 1] as usize;
                let attributes = u32::from_le_bytes([dir_data[offset + 4], dir_data[offset + 5], dir_data[offset + 6], dir_data[offset + 7]]);
                let is_directory = (attributes & 0x10) != 0;

                if secondary_count == 0 || i + 1 >= entry_count {
                    i += 1 + secondary_count;
                    continue;
                }

                let stream_offset = (i + 1) * 32;
                if dir_data[stream_offset] != 0xC0 {
                    i += 1 + secondary_count;
                    continue;
                }

                let general_flags = dir_data[stream_offset + 1];
                let contiguous = (general_flags & 0x02) != 0;
                let name_length = dir_data[stream_offset + 3] as usize;

                let size = u64::from_le_bytes([
                    dir_data[stream_offset + 8],
                    dir_data[stream_offset + 9],
                    dir_data[stream_offset + 10],
                    dir_data[stream_offset + 11],
                    dir_data[stream_offset + 12],
                    dir_data[stream_offset + 13],
                    dir_data[stream_offset + 14],
                    dir_data[stream_offset + 15],
                ]);

                let first_cluster = u32::from_le_bytes([dir_data[stream_offset + 20], dir_data[stream_offset + 21], dir_data[stream_offset + 22], dir_data[stream_offset + 23]]);

                let mut name_chars: Vec<u16> = Vec::new();
                for j in 0..secondary_count.saturating_sub(1) {
                    let name_entry_idx = i + 2 + j;
                    if name_entry_idx >= entry_count {
                        break;
                    }
                    let name_offset = name_entry_idx * 32;
                    if dir_data[name_offset] != 0xC1 {
                        break;
                    }
                    for k in 0..15 {
                        if name_chars.len() >= name_length {
                            break;
                        }
                        let char_offset = name_offset + 2 + k * 2;
                        if char_offset + 1 >= dir_data.len() {
                            break;
                        }
                        name_chars.push(u16::from_le_bytes([dir_data[char_offset], dir_data[char_offset + 1]]));
                    }
                }

                let name = String::from_utf16_lossy(&name_chars);
                let full_path = if path_prefix == "/" { format!("/{}", name) } else { format!("{}/{}", path_prefix, name) };

                let chain = if first_cluster < 2 {
                    Vec::new()
                } else if contiguous {
                    let num_clusters = if size == 0 { 0u32 } else { ((size + CLUSTER_SIZE as u64 - 1) / CLUSTER_SIZE as u64) as u32 };
                    (0..num_clusters).map(|i| first_cluster + i).collect()
                } else {
                    get_chain(fat, cluster_count, first_cluster)
                };

                if is_directory {
                    for &c in &chain {
                        metadata_clusters.insert(c);
                    }
                    if first_cluster >= 2 {
                        traverse_seekable(reader, fat, cluster_count, cluster_heap_offset_sectors, first_cluster, &full_path, files, metadata_clusters, base_offset)?;
                    }
                } else {
                    files.push(FileInfo { path: full_path, size, chain });
                }

                i += 1 + secondary_count;
            }
            0x81 => {
                let bitmap_cluster = u32::from_le_bytes([dir_data[offset + 20], dir_data[offset + 21], dir_data[offset + 22], dir_data[offset + 23]]);
                for &c in &get_chain(fat, cluster_count, bitmap_cluster) {
                    metadata_clusters.insert(c);
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok(())
}
