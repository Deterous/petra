use std::fmt;
use std::io::Read;

pub const HEADER_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartitionCode {
    Empty,
    Idstor,
    SLoader,
    Os0,
    Vs0,
    Vd0,
    Tm0,
    Ur0,
    Ux0,
    Gro0,
    Grw0,
    Ud0,
    Sa0,
    MediaID,
    Pd0,
    Unknown(u8),
}

impl PartitionCode {
    fn from_byte(b: u8) -> Self {
        match b {
            0x0 => PartitionCode::Empty,
            0x1 => PartitionCode::Idstor,
            0x2 => PartitionCode::SLoader,
            0x3 => PartitionCode::Os0,
            0x4 => PartitionCode::Vs0,
            0x5 => PartitionCode::Vd0,
            0x6 => PartitionCode::Tm0,
            0x7 => PartitionCode::Ur0,
            0x8 => PartitionCode::Ux0,
            0x9 => PartitionCode::Gro0,
            0xA => PartitionCode::Grw0,
            0xB => PartitionCode::Ud0,
            0xC => PartitionCode::Sa0,
            0xD => PartitionCode::MediaID,
            0xE => PartitionCode::Pd0,
            other => PartitionCode::Unknown(other),
        }
    }

    pub fn name(&self) -> Option<&'static str> {
        match self {
            PartitionCode::Empty => Some("empty"),
            PartitionCode::Idstor => Some("idstor"),
            PartitionCode::SLoader => Some("sloader"),
            PartitionCode::Os0 => Some("os"),
            PartitionCode::Vs0 => Some("vsh"),
            PartitionCode::Vd0 => Some("vshdata"),
            PartitionCode::Tm0 => Some("vtrm"),
            PartitionCode::Ur0 => Some("user"),
            PartitionCode::Ux0 => Some("userext"),
            PartitionCode::Gro0 => Some("gamero"),
            PartitionCode::Grw0 => Some("gamerw"),
            PartitionCode::Ud0 => Some("updater"),
            PartitionCode::Sa0 => Some("sysdata"),
            PartitionCode::MediaID => Some("mediaid"),
            PartitionCode::Pd0 => Some("pidata"),
            PartitionCode::Unknown(_) => None,
        }
    }
}

impl fmt::Display for PartitionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{}", name),
            None => {
                if let PartitionCode::Unknown(b) = self {
                    write!(f, "Unknown (0x{:02X})", b)
                } else {
                    unreachable!()
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileSystem {
    Fat16,
    ExFat,
    Raw,
    Unknown(u8),
}

impl FileSystem {
    fn from_byte(b: u8) -> Self {
        match b {
            0x06 => FileSystem::Fat16,
            0x07 => FileSystem::ExFat,
            0xDA => FileSystem::Raw,
            other => FileSystem::Unknown(other),
        }
    }
}

impl fmt::Display for FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileSystem::Fat16 => write!(f, "FAT16"),
            FileSystem::ExFat => write!(f, "exFAT"),
            FileSystem::Raw => write!(f, "Raw"),
            FileSystem::Unknown(code) => write!(f, "Unknown (0x{:02X})", code),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub offset: u32,
    pub size: u32,
    pub code: PartitionCode,
    pub filesystem: FileSystem,
    pub active: u8,
    pub flags: u32,
}

pub struct ImgHeader {
    pub raw: [u8; HEADER_SIZE],
    pub partitions: Vec<Partition>,
}

impl ImgHeader {
    pub fn version(&self) -> u32 {
        u32::from_le_bytes([self.raw[0x20], self.raw[0x21], self.raw[0x22], self.raw[0x23]])
    }

    pub fn image_size(&self) -> u32 {
        u32::from_le_bytes([self.raw[0x24], self.raw[0x25], self.raw[0x26], self.raw[0x27]])
    }

    pub fn print(&self, actual_file_size: u64, extract: bool) {
        if self.version() != 3 {
            println!("WARNING: Unexpected header version: {}", self.version());
        }

        let device_size_bytes = self.image_size() as u64 * 512;
        if actual_file_size > device_size_bytes {
            if extract {
                println!("Trimmed skeleton to device size: {} bytes ({} blocks)", device_size_bytes, self.image_size());
            } else {
                println!("Image larger than device size: {} bytes ({} blocks)", device_size_bytes, self.image_size());
            }
        } else if actual_file_size < device_size_bytes {
            println!("WARNING: File is smaller than device size: header says {} bytes ({} blocks), actual file is {} bytes", device_size_bytes, self.image_size(), actual_file_size);
        }

        if !is_region_zero(&self.raw, 0x28, 0x4F) {
            println!("WARNING: 0x28-0x4F contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x160, 0x1BD) {
            println!("WARNING: 0x160-0x1BD contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1BE, 0x1CD) {
            println!("WARNING: MBR partition 1 (0x1BE-0x1CD) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1CE, 0x1DD) {
            println!("WARNING: MBR partition 2 (0x1CE-0x1DD) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1DE, 0x1ED) {
            println!("WARNING: MBR partition 3 (0x1DE-0x1ED) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1EE, 0x1FD) {
            println!("WARNING: MBR partition 4 (0x1EE-0x1FD) contains non-zero data");
        }

        if self.raw[0x1FE] != 0x55 || self.raw[0x1FF] != 0xAA {
            println!("WARNING: Invalid MBR boot signature at 0x1FE: expected 55 AA, found {:02X} {:02X}", self.raw[0x1FE], self.raw[0x1FF]);
        }

        for (i, entry) in self.partitions.iter().enumerate() {
            let active_str = match entry.active {
                0x00 => "no".to_string(),
                0x01 => "yes".to_string(),
                x => format!("0x{:02X}", x),
            };
            let flags_str = match entry.flags {
                0x555 => "read-only (0x555)".to_string(),
                0xFFF => "read-write (0xFFF)".to_string(),
                x => format!("0x{:03X}", x),
            };
            println!(
                "Partition {}: offset=0x{:X}, length=0x{:X}, type={}, fs={}, active={}, flags={}",
                i,
                entry.offset as u64 * 512,
                entry.size as u64 * 512,
                entry.code,
                entry.filesystem,
                active_str,
                flags_str
            );
        }
    }
}

fn is_region_zero(data: &[u8], start: usize, end: usize) -> bool {
    data[start..=end].iter().all(|&b| b == 0)
}

fn parse_partitions(header_raw: &[u8; HEADER_SIZE], file_size: u64) -> Result<Vec<Partition>, String> {
    const ENTRY_SIZE: usize = 0x11;
    const MAX_ENTRIES: usize = 16;
    const TABLE_START: usize = 0x50;

    let mut entries = Vec::new();

    for i in 0..MAX_ENTRIES {
        let base = TABLE_START + i * ENTRY_SIZE;
        let entry_bytes = &header_raw[base..base + ENTRY_SIZE];

        let offset = u32::from_le_bytes([entry_bytes[0x0], entry_bytes[0x1], entry_bytes[0x2], entry_bytes[0x3]]);

        if offset == 0 {
            break;
        }

        let size = u32::from_le_bytes([entry_bytes[0x4], entry_bytes[0x5], entry_bytes[0x6], entry_bytes[0x7]]);

        let code = PartitionCode::from_byte(entry_bytes[0x8]);
        let filesystem = FileSystem::from_byte(entry_bytes[0x9]);
        let active = entry_bytes[0xA];

        let flags = u32::from_le_bytes([entry_bytes[0xB], entry_bytes[0xC], entry_bytes[0xD], entry_bytes[0xE]]);

        let unknown = &entry_bytes[0xF..0x11];
        if unknown[0] != 0 || unknown[1] != 0 {
            println!("WARNING: Partition entry {} has non-zero bytes at offset 0xF: {:02X} {:02X}", i, unknown[0], unknown[1]);
        }

        entries.push(Partition { offset, size, code, filesystem, active, flags });
    }

    if entries.len() >= 2 {
        let mut sorted: Vec<&Partition> = entries.iter().collect();
        sorted.sort_by_key(|e| e.offset);

        for i in 0..sorted.len() - 1 {
            let current_end = sorted[i].offset.saturating_add(sorted[i].size);
            if current_end > sorted[i + 1].offset {
                return Err(format!("ERROR: Partition overlap detected between partitions at offset 0x{:04X} and 0x{:04X}", sorted[i].offset, sorted[i + 1].offset));
            }
        }
    }

    for entry in &entries {
        let end = (entry.offset as u64 + entry.size as u64) * 512;
        if end > file_size {
            return Err(format!("ERROR: Partition at offset 0x{:04X} extends beyond file size (end: {} bytes, file: {} bytes)", entry.offset, end, file_size));
        }
    }

    Ok(entries)
}

pub fn partition_names(partitions: &[Partition], extractable: fn(&FileSystem) -> bool) -> Vec<Option<String>> {
    let mut code_totals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for p in partitions.iter().filter(|p| extractable(&p.filesystem)) {
        *code_totals.entry(p.code.name().unwrap_or("partition").to_string()).or_insert(0) += 1;
    }
    let extractable_count: usize = code_totals.values().sum();
    let mut code_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut index: usize = 0;
    partitions
        .iter()
        .map(|p| {
            if !extractable(&p.filesystem) {
                return None;
            }
            let base_name = p.code.name().unwrap_or("partition").to_string();
            let name = if extractable_count > 1 {
                let total = *code_totals.get(&base_name).unwrap_or(&1);
                let count = code_counts.entry(base_name.clone()).or_insert(0);
                let folder_name = if total > 1 { format!("{}{}", base_name, count) } else { base_name.clone() };
                *count += 1;
                folder_name
            } else {
                base_name
            };
            index += 1;
            Some(name)
        })
        .collect()
}

pub fn parse(reader: &mut impl Read, file_size: u64) -> Result<ImgHeader, String> {
    const MAGIC: &[u8; 32] = b"Sony Computer Entertainment Inc.";

    let mut raw = [0u8; HEADER_SIZE];

    reader.read_exact(&mut raw).map_err(|e| format!("ERROR: Failed to read image header: {}", e))?;

    if &raw[0x00..0x20] != MAGIC.as_slice() {
        return Err("ERROR: Invalid psvita image file".to_string());
    }

    let partitions = parse_partitions(&raw, file_size)?;

    Ok(ImgHeader { raw, partitions })
}
