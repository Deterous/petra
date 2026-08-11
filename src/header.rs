use std::fmt;
use std::io::Read;

pub const HEADER_SIZE: usize = 512;

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
    pub code: u8,
    pub filesystem: FileSystem,
    pub active: u8,
    pub flags: u32,
}

pub struct PsvHeader {
    pub raw: [u8; HEADER_SIZE],
    pub partitions: Vec<Partition>,
}

impl PsvHeader {
    pub fn version(&self) -> u32 {
        u32::from_le_bytes([self.raw[0x20], self.raw[0x21], self.raw[0x22], self.raw[0x23]])
    }

    pub fn image_size(&self) -> u32 {
        u32::from_le_bytes([self.raw[0x24], self.raw[0x25], self.raw[0x26], self.raw[0x27]])
    }

    pub fn print(&self, actual_file_size: u64) {
        println!("Version: {}", self.version());

        let device_size_bytes = self.image_size() as u64 * 512;
        if device_size_bytes != actual_file_size {
            eprintln!("WARNING: Device size mismatch: header says {} bytes ({} blocks), actual file is {} bytes", device_size_bytes, self.image_size(), actual_file_size);
        }

        if !is_region_zero(&self.raw, 0x28, 0x4F) {
            eprintln!("WARNING: 0x28-0x4F contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x160, 0x1BD) {
            eprintln!("WARNING: 0x160-0x1BD contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1BE, 0x1CD) {
            eprintln!("WARNING: MBR partition 1 (0x1BE-0x1CD) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1CE, 0x1DD) {
            eprintln!("WARNING: MBR partition 2 (0x1CE-0x1DD) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1DE, 0x1ED) {
            eprintln!("WARNING: MBR partition 3 (0x1DE-0x1ED) contains non-zero data");
        }

        if !is_region_zero(&self.raw, 0x1EE, 0x1FD) {
            eprintln!("WARNING: MBR partition 4 (0x1EE-0x1FD) contains non-zero data");
        }

        if self.raw[0x1FE] != 0x55 || self.raw[0x1FF] != 0xAA {
            eprintln!("WARNING: Invalid MBR boot signature at 0x1FE: expected 55 AA, found {:02X} {:02X}", self.raw[0x1FE], self.raw[0x1FF]);
        }

        for (i, entry) in self.partitions.iter().enumerate() {
            println!("Partition {}: code=0x{:02X}, type={}, active=0x{:02X}, flags=0x{:08X}", i, entry.code, entry.filesystem, entry.active, entry.flags);
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

        let code = entry_bytes[0x8];
        let filesystem = FileSystem::from_byte(entry_bytes[0x9]);
        let active = entry_bytes[0xA];

        let flags = u32::from_le_bytes([entry_bytes[0xB], entry_bytes[0xC], entry_bytes[0xD], entry_bytes[0xE]]);

        let unknown = &entry_bytes[0xF..0x11];
        if unknown[0] != 0 || unknown[1] != 0 {
            eprintln!("WARNING: Partition entry {} has non-zero bytes at offset 0xF: {:02X} {:02X}", i, unknown[0], unknown[1]);
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

pub fn parse(reader: &mut impl Read, file_size: u64) -> Result<PsvHeader, String> {
    const MAGIC: &[u8; 32] = b"Sony Computer Entertainment Inc.";

    let mut raw = [0u8; HEADER_SIZE];

    reader.read_exact(&mut raw).map_err(|e| format!("ERROR: Failed to read PSV header: {}", e))?;

    if &raw[0x00..0x20] != MAGIC.as_slice() {
        return Err("ERROR: Invalid PSV file".to_string());
    }

    let partitions = parse_partitions(&raw, file_size)?;

    Ok(PsvHeader { raw, partitions })
}
