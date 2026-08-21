use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

#[derive(Debug)]
pub struct HashEntry {
    pub sha256: String,
    pub offset: u64,
    pub size: u64,
    pub path: String,
}

pub const HASH_FILE_HEADER: &str = "sha256\toffset(sectors)\tsize(bytes)\tpath";

pub fn write_hash_file(path: &Path, entries: &[HashEntry]) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "{}", HASH_FILE_HEADER)?;
    for entry in entries {
        writeln!(writer, "{}\t{}\t{}\t{}", entry.sha256, entry.offset, entry.size, entry.path)?;
    }
    writer.flush()?;
    Ok(())
}

pub fn read_hash_file(path: &Path) -> Result<Vec<HashEntry>, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("ERROR: Failed to read hash file: {}", e))?;

    let mut entries = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        if line_num == 0 && line.starts_with("sha256\t") {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 || parts.len() > 4 {
            return Err(format!("ERROR: Invalid hash file at line {}: \"{}\"", line_num + 1, line));
        }
        let offset: u64 = parts[1].parse().map_err(|_| format!("ERROR: Invalid offset at line {}: \"{}\"", line_num + 1, line))?;
        let size: u64 = parts[2].parse().map_err(|_| format!("ERROR: Invalid size at line {}: \"{}\"", line_num + 1, line))?;
        let entry_path = if parts.len() == 4 { parts[3].to_string() } else { String::new() };

        entries.push(HashEntry { sha256: parts[0].to_string(), offset, size, path: entry_path });
    }

    Ok(entries)
}

pub fn validate_hash_bounds(entries: &[HashEntry], file_size: u64) -> Result<(), String> {
    for entry in entries {
        let byte_offset = entry.offset * 512;
        if byte_offset >= file_size {
            return Err(format!("ERROR: Offset out of bounds: offset {} (byte 0x{:X}) exceeds file size {} (0x{:X})", entry.offset, byte_offset, file_size, file_size));
        }

        if byte_offset + entry.size > file_size {
            return Err(format!(
                "ERROR: Entry extends beyond file: offset {} (byte 0x{:X}) + size {} = 0x{:X}, but file size is {} (0x{:X})",
                entry.offset,
                byte_offset,
                entry.size,
                byte_offset + entry.size,
                file_size,
                file_size
            ));
        }
    }

    Ok(())
}
