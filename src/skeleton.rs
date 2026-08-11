use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use zstd::{Decoder, Encoder};

const CHUNK_SIZE: usize = 64 * 1024;

pub fn read_image_size(skeleton_path: &Path) -> Result<u64, String> {
    let input = File::open(skeleton_path).map_err(|e| format!("ERROR: Failed to open skeleton: {}", e))?;
    let reader = BufReader::new(input);
    let mut decompressor = Decoder::new(reader).map_err(|e| format!("ERROR: Failed to init zstd decoder: {}", e))?;

    let mut buf = [0u8; 0x28];
    decompressor.read_exact(&mut buf).map_err(|e| format!("ERROR: Failed to read header from skeleton: {}", e))?;

    let image_blocks = u32::from_le_bytes([buf[0x24], buf[0x25], buf[0x26], buf[0x27]]);
    Ok(image_blocks as u64 * 512)
}

pub fn read_image_size_raw(skeleton_path: &Path) -> Result<u64, String> {
    let mut input = File::open(skeleton_path).map_err(|e| format!("ERROR: Failed to open skeleton: {}", e))?;

    let mut buf = [0u8; 0x28];
    input.read_exact(&mut buf).map_err(|e| format!("ERROR: Failed to read header from skeleton: {}", e))?;

    let image_blocks = u32::from_le_bytes([buf[0x24], buf[0x25], buf[0x26], buf[0x27]]);
    Ok(image_blocks as u64 * 512)
}

pub fn decompress_skeleton(skeleton_path: &Path, output_path: &Path) -> std::io::Result<()> {
    let input = File::open(skeleton_path)?;
    let reader = BufReader::new(input);
    let mut decompressor = Decoder::new(reader)?;

    let output = File::create(output_path)?;
    let mut writer = BufWriter::new(output);

    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = decompressor.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    writer.flush()?;
    Ok(())
}

pub struct SkeletonWriter {
    compressor: Encoder<'static, BufWriter<File>>,
}

impl SkeletonWriter {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        let buf_writer = BufWriter::new(file);
        let compressor = Encoder::new(buf_writer, 3)?;
        Ok(Self { compressor })
    }

    pub fn write_bytes(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.compressor.write_all(data)
    }

    pub fn write_zeros(&mut self, count: u64) -> std::io::Result<()> {
        let zeros = [0u8; CHUNK_SIZE];

        let mut remainder = count;
        while remainder > 0 {
            let to_write = remainder.min(CHUNK_SIZE as u64) as usize;
            self.compressor.write_all(&zeros[..to_write])?;
            remainder -= to_write as u64;
        }
        Ok(())
    }

    pub fn finish(self) -> std::io::Result<()> {
        self.compressor.finish()?;
        Ok(())
    }
}
