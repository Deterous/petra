use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::common::{self, BLACKFIN_OFFSET, BLACKFIN_SIZE, BLOCK_SIZE, HEADER_SKIP, LIC1_OFFSET, LIC1_SIZE, LIC2_OFFSET, LIC2_SIZE, UNK_OFFSET, UNK_SIZE};
use crate::header;

pub fn run(source: &Path) -> Result<(), String> {
    if source.is_dir() {
        return common::iter_image_files(source, run);
    }

    let file = File::open(source).map_err(|e| format!("ERROR: Failed to open {}: {}", source.display(), e))?;
    let file_size = file.metadata().map_err(|e| format!("ERROR: {}", e))?.len();
    common::check_file_size(file_size, source)?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(|e| format!("ERROR: Failed to read magic: {}", e))?;
    let has_header = &magic == b"PSV\0" || &magic == b"VCI\0";
    let data_offset: u64 = if has_header { HEADER_SKIP } else { 0 };
    let data_size = file_size - data_offset;

    reader.seek(SeekFrom::Start(data_offset)).map_err(|e| format!("ERROR: {}", e))?;
    let img_header = header::parse(&mut reader, data_size)?;

    let blackfin_start = data_offset + BLACKFIN_OFFSET;
    let blackfin_end = blackfin_start + BLACKFIN_SIZE;
    let unk_start = data_offset + UNK_OFFSET;
    let unk_end = unk_start + UNK_SIZE;
    let (rif_start, rif_size) = common::find_rif(&mut reader, &img_header, data_offset)?.ok_or("ERROR: No .rif file found in image filesystem")?;
    let basename = source.with_extension("");

    reader.seek(SeekFrom::Start(blackfin_start)).map_err(|e| format!("ERROR: {}", e))?;
    let mut blackfin_buf = vec![0u8; BLACKFIN_SIZE as usize];
    reader.read_exact(&mut blackfin_buf).map_err(|e| format!("ERROR: Failed to read BlackFin region: {}", e))?;
    let has_blackfin = common::save_blackfin(&blackfin_buf, &basename)?;

    let first_partition_start = data_offset + img_header.partitions.iter().map(|p| p.offset as u64 * BLOCK_SIZE).min().unwrap_or(0);
    let mut scan_pos = data_offset + BLOCK_SIZE;
    while scan_pos < first_partition_start {
        let skip_end = if scan_pos >= unk_start && scan_pos < unk_end {
            Some(unk_end)
        } else if has_blackfin && scan_pos >= blackfin_start && scan_pos < blackfin_end {
            Some(blackfin_end)
        } else {
            None
        };

        if let Some(end) = skip_end {
            scan_pos = end;
            continue;
        }

        let next_skip = [unk_start, if has_blackfin { blackfin_start } else { u64::MAX }, first_partition_start].iter().copied().filter(|&x| x > scan_pos).min().unwrap();

        let len = (next_skip - scan_pos) as usize;
        let mut buf = vec![0u8; len];
        reader.seek(SeekFrom::Start(scan_pos)).map_err(|e| format!("ERROR: {}", e))?;
        reader.read_exact(&mut buf).map_err(|e| format!("ERROR: Failed to read gap at 0x{:X}: {}", scan_pos, e))?;
        if buf.iter().any(|&b| b != 0) {
            println!("WARNING: Unexpected non-zero data in gap 0x{:X}-0x{:X}", scan_pos, next_skip);
        }
        scan_pos = next_skip;
    }

    if has_header {
        reader.seek(SeekFrom::Start(0)).map_err(|e| format!("ERROR: {}", e))?;
        let mut hdr = vec![0u8; HEADER_SKIP as usize];
        reader.read_exact(&mut hdr).map_err(|e| format!("ERROR: Failed to read header: {}", e))?;
        common::save_hdr(&hdr, &basename)?;
    }

    reader.seek(SeekFrom::Start(unk_start)).map_err(|e| format!("ERROR: {}", e))?;
    let mut unk_buf = vec![0u8; UNK_SIZE as usize];
    reader.read_exact(&mut unk_buf).map_err(|e| format!("ERROR: Failed to read unk region: {}", e))?;
    let has_unk = common::save_unk(&unk_buf, &basename)?;

    reader.seek(SeekFrom::Start(rif_start)).map_err(|e| format!("ERROR: {}", e))?;
    let mut rif = vec![0u8; rif_size as usize];
    reader.read_exact(&mut rif).map_err(|e| format!("ERROR: Failed to read rif: {}", e))?;
    let has_rif = common::save_rif(&rif, &basename)?;

    if !has_header && !has_blackfin && !has_unk && !has_rif {
        println!("Image already clean: {}", source.display());
        return Ok(());
    }

    let img_end = data_offset + img_header.image_size() as u64 * BLOCK_SIZE;
    let has_footer = if file_size > img_end {
        let footer_len = (file_size - img_end) as usize;
        let mut footer_buf = vec![0u8; footer_len];
        reader.seek(SeekFrom::Start(img_end)).map_err(|e| format!("ERROR: {}", e))?;
        reader.read_exact(&mut footer_buf).map_err(|e| format!("ERROR: Failed to read footer: {}", e))?;
        common::save_footer(&footer_buf, &basename)?
    } else {
        false
    };

    drop(reader);
    let mut file = File::options().read(true).write(true).open(source).map_err(|e| format!("ERROR: Failed to open {} for writing: {}", source.display(), e))?;
    let (crc, md5, sha1, sha256) = common::hash_file(&mut file, file_size)?;
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    if has_header {
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        let mut read_pos = HEADER_SKIP;
        let mut write_pos = 0u64;
        loop {
            file.seek(SeekFrom::Start(read_pos)).map_err(|e| format!("ERROR: {}", e))?;
            let n = file.read(&mut buf).map_err(|e| format!("ERROR: Failed to read during header removal: {}", e))?;
            if n == 0 {
                break;
            }
            file.seek(SeekFrom::Start(write_pos)).map_err(|e| format!("ERROR: {}", e))?;
            file.write_all(&buf[..n]).map_err(|e| format!("ERROR: Failed to write during header removal: {}", e))?;
            read_pos += n as u64;
            write_pos += n as u64;
        }
        file.set_len(file_size - HEADER_SKIP).map_err(|e| format!("ERROR: Failed to truncate file: {}", e))?;
    }
    if has_unk {
        common::zero_range(&mut file, unk_start - data_offset, UNK_SIZE)?;
    }
    if has_blackfin {
        common::zero_range(&mut file, blackfin_start - data_offset, BLACKFIN_SIZE)?;
    }
    if has_rif {
        common::zero_range(&mut file, rif_start - data_offset + LIC1_OFFSET, LIC1_SIZE)?;
        common::zero_range(&mut file, rif_start - data_offset + LIC2_OFFSET, LIC2_SIZE)?;
    }
    let trimmed_end = img_end - data_offset;
    let current_len = if has_header { file_size - HEADER_SKIP } else { file_size };
    if !has_footer && current_len > trimmed_end {
        file.set_len(trimmed_end).map_err(|e| format!("ERROR: Failed to trim file to device size: {}", e))?;
    }
    file.flush().map_err(|e| format!("ERROR: {}", e))?;

    let stripped_size = file.metadata().map_err(|e| format!("ERROR: {}", e))?.len();
    let (stripped_crc, stripped_md5, stripped_sha1, stripped_sha256) = common::hash_file(&mut file, stripped_size)?;
    drop(file);

    let stripped_path = source.with_extension("img");
    if source != stripped_path {
        std::fs::rename(source, &stripped_path).map_err(|e| format!("ERROR: Failed to rename to {}: {}", stripped_path.display(), e))?;
        println!("Renamed: {} -> {}", source.display(), stripped_path.display());
    }
    let stripped_name = if source.extension().and_then(|e| e.to_str()) == Some("img") {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        format!("{}_stripped.img", stem)
    } else {
        stripped_path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    };
    let dat_path = basename.with_extension("dat");
    common::save_dat(&[
        (&name, file_size, crc, md5.as_str(), sha1.as_str(), sha256.as_str()),
        (&stripped_name, stripped_size, stripped_crc, stripped_md5.as_str(), stripped_sha1.as_str(), stripped_sha256.as_str()),
    ], &dat_path)?;

    println!("Stripped: {}", stripped_path.display());
    Ok(())
}
