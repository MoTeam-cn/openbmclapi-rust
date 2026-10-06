//! Avro binary decoding for the master file list.
//!
//! The master serialises `FileList` with `avsc.Type.forSchema` and the agent
//! decodes the *raw* Avro binary encoding (no container file header):
//!
//! ```text
//! array<record { path: string, hash: string, size: long, mtime: long }>
//! ```
//!
//! Hand-rolling the decoder keeps the agent free of an Avro dependency and is
//! significantly faster than a generic reader for this fixed shape.

use crate::error::{Error, Result};
use crate::types::FileInfo;

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn read_varint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            if self.pos >= self.buf.len() {
                return Err(Error::Other("truncated avro varint".into()));
            }
            let byte = self.buf[self.pos];
            self.pos += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift > 63 {
                return Err(Error::Other("avro varint overflow".into()));
            }
        }
    }

    fn read_long(&mut self) -> Result<i64> {
        let raw = self.read_varint()?;
        Ok(((raw >> 1) as i64) ^ -((raw & 1) as i64))
    }

    fn read_slice(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| Error::Other("avro length overflow".into()))?;
        if end > self.buf.len() {
            return Err(Error::Other("truncated avro data".into()));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_string(&mut self) -> Result<String> {
        let len = self.read_long()?;
        if len < 0 {
            return Err(Error::Other("negative avro string length".into()));
        }
        let bytes = self.read_slice(len as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| Error::Other(format!("invalid utf-8 in avro string: {e}")))
    }

    fn read_record(&mut self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.read_string()?,
            hash: self.read_string()?,
            size: self.read_long()?,
            mtime: self.read_long()?,
        })
    }
}

/// Decode a raw Avro `array<FileListEntry>` payload.
pub fn decode(buf: &[u8]) -> Result<Vec<FileInfo>> {
    let mut reader = Reader::new(buf);
    let mut files = Vec::new();
    loop {
        let count = reader.read_long()?;
        if count == 0 {
            break;
        }
        if count < 0 {
            // Negative counts are followed by the block size in bytes.
            let block_len = reader.read_long()?;
            if block_len < 0 {
                return Err(Error::Other("negative avro block size".into()));
            }
            for _ in 0..(-count) {
                files.push(reader.read_record()?);
            }
        } else {
            for _ in 0..count {
                files.push(reader.read_record()?);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_long(out: &mut Vec<u8>, value: i64) {
        let mut zig = ((value << 1) ^ (value >> 63)) as u64;
        loop {
            let mut byte = (zig & 0x7f) as u8;
            zig >>= 7;
            if zig != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if zig == 0 {
                break;
            }
        }
    }

    fn write_string(out: &mut Vec<u8>, value: &str) {
        write_long(out, value.len() as i64);
        out.extend_from_slice(value.as_bytes());
    }

    #[test]
    fn decodes_single_block() {
        let mut buf = Vec::new();
        write_long(&mut buf, 2);
        for (path, hash, size, mtime) in [
            ("/download/aa", "aaa", 10i64, 1i64),
            ("/download/bb", "bbb", 20, 2),
        ] {
            write_string(&mut buf, path);
            write_string(&mut buf, hash);
            write_long(&mut buf, size);
            write_long(&mut buf, mtime);
        }
        write_long(&mut buf, 0);
        let files = decode(&buf).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "/download/aa");
        assert_eq!(files[1].size, 20);
    }

    #[test]
    fn decodes_negative_block() {
        let mut buf = Vec::new();
        write_long(&mut buf, -1);
        write_long(&mut buf, 99); // block byte size (ignored)
        write_string(&mut buf, "/download/cc");
        write_string(&mut buf, "ccc");
        write_long(&mut buf, 30);
        write_long(&mut buf, 3);
        write_long(&mut buf, 0);
        let files = decode(&buf).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hash, "ccc");
    }
}
