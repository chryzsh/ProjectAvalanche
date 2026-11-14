use std::io::Read as _;

use crate::reader::Reader;
use crate::{Error, Result};

mod pack;
mod pose;
mod record;

pub use pack::*;
pub use pose::*;
pub use record::*;

pub const CONTAINER_MAGIC: &[u8; 4] = b"GCMP";
pub const MAGIC: &[u8; 4] = b"GHST";
pub const VERSION: u16 = 0x1A;
pub const BUILD: u32 = 0x6DA1D;
pub const DESCRIPTOR_SIZE: usize = 0x24;
pub const USER_HEADER_OFFSET: usize = 0x1C;
pub const USER_HEADER_SIZE: usize = 0x3C;
pub const TABLE_OFFSET: usize = USER_HEADER_OFFSET + USER_HEADER_SIZE;
pub const GAMERTAG_BYTES: usize = 0x32;
pub const POSI: [u8; 4] = *b"POSI";
pub const GMPY: [u8; 4] = *b"GMPY";
pub const SCOR: [u8; 4] = *b"SCOR";

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
    }
    !c
}

pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::be(data);
    let size = r.u32()? as usize;
    r.expect(CONTAINER_MAGIC)?;
    let body = &data[8..];
    let mut z = flate2::read::ZlibDecoder::new(body);
    let mut out = Vec::with_capacity(size);
    z.read_to_end(&mut out).map_err(|e| Error::Decompress(format!("GCMP zlib stream: {e}")))?;
    if z.total_in() as usize != body.len() {
        return Err(Error::invalid(format!(
            "GCMP: {} bytes after the zlib stream",
            body.len() - z.total_in() as usize
        )));
    }
    if out.len() != size {
        return Err(Error::invalid(format!("GCMP: inflated to {} bytes, header says {size}", out.len())));
    }
    Ok(out)
}

// TODO(guess): the zlib level of the game's GCMP writer; known: the disc ghosts inflate through `decompress` whatever the level, the reader checks only the size; settle: the compressor call under `GhostFile_WriteHeader` 0x8253DC98's caller.
pub fn compress(image: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = Vec::with_capacity(image.len() / 2 + 8);
    out.extend_from_slice(&(image.len() as u32).to_be_bytes());
    out.extend_from_slice(CONTAINER_MAGIC);
    let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::default());
    z.write_all(image).expect("writing to a Vec");
    z.finish().expect("writing to a Vec")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserHeader {
    pub gamertag: String,
    pub gear_flags: u16,
    pub score: u32,
    pub build: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub tag: [u8; 4],
    pub capacity: u32,
    pub element_size: u32,
    pub alignment: u32,
    pub interval: f32,
    pub times_offset: u32,
    pub data_offset: u32,
    pub data_size: u32,
    pub count: u32,
    pub times: Vec<f32>,
    pub data: Vec<u8>,
}

impl Stream {
    pub fn new(tag: [u8; 4], element_size: u32, alignment: u32, interval: f32, times: Vec<f32>, data: Vec<u8>) -> Self {
        let count = times.len() as u32;
        Stream {
            tag,
            capacity: count,
            element_size,
            alignment,
            interval,
            times_offset: 0,
            data_offset: 0,
            data_size: count * element_size,
            count,
            times,
            data,
        }
    }

    pub fn element(&self, i: usize) -> &[u8] {
        let n = self.element_size as usize;
        &self.data[i * n..(i + 1) * n]
    }
}

fn align_up(n: usize, a: usize) -> usize {
    n.div_ceil(a.max(1)) * a.max(1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GhostFile {
    pub crc: u32,
    pub version: u16,
    pub unk_1a: u16,
    pub header: UserHeader,
    pub streams: Vec<Stream>,
}

impl GhostFile {
    pub fn from_container(data: &[u8]) -> Result<Self> {
        Self::parse(&decompress(data)?)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let crc = r.u32()?;
        r.expect(MAGIC)?;
        if crc32(&data[4..]) != crc {
            return Err(Error::invalid(format!("GHST: CRC {crc:#010x} does not match {:#010x}", crc32(&data[4..]))));
        }
        let (table, count, user_offset, user_size) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let (version, unk_1a) = (r.u16()?, r.u16()?);
        if version != VERSION {
            return Err(Error::invalid(format!("GHST: version {version:#x}, the reader takes {VERSION:#x}")));
        }
        if user_offset != 0x1C || user_size != 0x3C || table != user_offset + user_size {
            return Err(Error::invalid(format!(
                "GHST: user header at {user_offset:#x} size {user_size:#x}, table at {table:#x} (0x1C / 0x3C / 0x58 on the disc)"
            )));
        }
        r.seek(user_offset as usize)?;
        let name = r.bytes(0x32)?;
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let gamertag = String::from_utf8_lossy(&name[..end]).into_owned();
        let header = UserHeader { gamertag, gear_flags: r.u16()?, score: r.u32()?, build: r.u32()? };
        let mut streams = Vec::with_capacity(count as usize);
        let mut image_end = table as usize + count as usize * DESCRIPTOR_SIZE;
        for i in 0..count as usize {
            r.seek(table as usize + i * DESCRIPTOR_SIZE)?;
            let tag = r.array::<4>()?;
            let (capacity, element_size, alignment) = (r.u32()?, r.u32()?, r.u32()?);
            let interval = r.f32()?;
            let (times_offset, data_offset, data_size, count) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            let label = String::from_utf8_lossy(&tag).into_owned();
            if count > capacity || u64::from(capacity) * u64::from(element_size) != u64::from(data_size) {
                return Err(Error::invalid(format!(
                    "GHST {label}: count {count}, capacity {capacity} × {element_size} B vs data size {data_size}"
                )));
            }
            if alignment == 0 || data_offset % alignment != 0 {
                return Err(Error::invalid(format!("GHST {label}: data at {data_offset:#x}, alignment {alignment}")));
            }
            let times_end = times_offset as usize + capacity as usize * 4;
            let data_end = data_offset as usize + data_size as usize;
            if times_end > data.len() || data_end > data.len() {
                return Err(Error::invalid(format!("GHST {label}: regions past the image ({} bytes)", data.len())));
            }
            image_end = image_end.max(times_end).max(data_end);
            r.seek(times_offset as usize)?;
            let times = (0..count).map(|_| r.f32()).collect::<Result<Vec<_>>>()?;
            let used = count as usize * element_size as usize;
            let data = data[data_offset as usize..data_offset as usize + used].to_vec();
            streams.push(Stream {
                tag,
                capacity,
                element_size,
                alignment,
                interval,
                times_offset,
                data_offset,
                data_size,
                count,
                times,
                data,
            });
        }
        if image_end != data.len() {
            return Err(Error::invalid(format!("GHST: regions end at {image_end:#x}, image is {:#x}", data.len())));
        }
        Ok(Self { crc, version, unk_1a, header, streams })
    }

    // TODO(guess): the region order (each stream's times then its data, data aligned to the descriptor's alignment) and a capacity equal to the count; known: `GhostFile_WriteHeader` 0x8253DC98 writes the header fields (table 0x58, user 0x1C / 0x3C, version 0x1A, build 0x6DA1D, the count from the descriptors' span) and the CRC over the image after the first word, the reader takes any order that ends with the image; settle: the stream allocation under `GhostRecorder_Init` 0x824BE0D8 (0x824B6970 per stream).
    pub fn write(&self) -> Vec<u8> {
        let count = self.streams.len();
        let mut out = vec![0u8; TABLE_OFFSET + count * DESCRIPTOR_SIZE];
        out[4..8].copy_from_slice(MAGIC);
        out[8..12].copy_from_slice(&(TABLE_OFFSET as u32).to_be_bytes());
        out[12..16].copy_from_slice(&(count as u32).to_be_bytes());
        out[16..20].copy_from_slice(&(USER_HEADER_OFFSET as u32).to_be_bytes());
        out[20..24].copy_from_slice(&(USER_HEADER_SIZE as u32).to_be_bytes());
        out[24..26].copy_from_slice(&self.version.to_be_bytes());
        out[26..28].copy_from_slice(&self.unk_1a.to_be_bytes());
        let name = self.header.gamertag.as_bytes();
        let n = name.len().min(GAMERTAG_BYTES - 1);
        out[USER_HEADER_OFFSET..USER_HEADER_OFFSET + n].copy_from_slice(&name[..n]);
        let g = USER_HEADER_OFFSET + GAMERTAG_BYTES;
        out[g..g + 2].copy_from_slice(&self.header.gear_flags.to_be_bytes());
        out[g + 2..g + 6].copy_from_slice(&self.header.score.to_be_bytes());
        out[g + 6..g + 10].copy_from_slice(&self.header.build.to_be_bytes());
        for (i, s) in self.streams.iter().enumerate() {
            let times_offset = align_up(out.len(), 4);
            out.resize(times_offset, 0);
            for t in &s.times {
                out.extend_from_slice(&t.to_be_bytes());
            }
            out.resize(times_offset + s.capacity as usize * 4, 0);
            let data_offset = align_up(out.len(), s.alignment as usize);
            out.resize(data_offset, 0);
            out.extend_from_slice(&s.data);
            out.resize(data_offset + s.data_size as usize, 0);
            let d = TABLE_OFFSET + i * DESCRIPTOR_SIZE;
            out[d..d + 4].copy_from_slice(&s.tag);
            let words = [
                s.capacity,
                s.element_size,
                s.alignment,
                s.interval.to_bits(),
                times_offset as u32,
                data_offset as u32,
                s.data_size,
                s.count,
            ];
            for (k, w) in words.iter().enumerate() {
                let at = d + 4 + k * 4;
                out[at..at + 4].copy_from_slice(&w.to_be_bytes());
            }
        }
        let crc = crc32(&out[4..]);
        out[..4].copy_from_slice(&crc.to_be_bytes());
        out
    }

    pub fn stream(&self, tag: [u8; 4]) -> Option<&Stream> {
        self.streams.iter().find(|s| s.tag == tag)
    }

    fn typed(&self, tag: [u8; 4], size: usize) -> Result<&Stream> {
        let s =
            self.stream(tag).ok_or_else(|| Error::invalid(format!("no {} stream", String::from_utf8_lossy(&tag))))?;
        if s.element_size as usize != size {
            return Err(Error::invalid(format!("{}: {} B elements", String::from_utf8_lossy(&tag), s.element_size)));
        }
        Ok(s)
    }

    pub fn positions(&self) -> Result<Vec<[f32; 3]>> {
        let s = self.typed(POSI, 12)?;
        (0..s.count as usize).map(|i| Reader::be(s.element(i)).vec3()).collect()
    }

    pub fn scores(&self) -> Result<Vec<f32>> {
        let s = self.typed(SCOR, 4)?;
        (0..s.count as usize).map(|i| Reader::be(s.element(i)).f32()).collect()
    }

    pub fn poses(&self) -> Result<Vec<PoseBlock>> {
        let s = self.typed(GMPY, POSE_BLOCK_SIZE)?;
        (0..s.count as usize).map(|i| PoseBlock::parse(s.element(i))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_bzip2_check() {
        assert_eq!(crc32(b"123456789"), 0xFC89_1918);
    }
}
