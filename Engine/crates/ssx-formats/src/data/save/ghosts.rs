use super::crc32_bzip2;
use crate::reader::Reader;
use crate::{Error, Result};

pub const VERSION: u32 = 0xC;
pub const INDEX_BUFFER_SIZE: usize = 0x3C00;
pub const GHOST_BUFFER_SIZE: usize = 0x3E800;
pub const MAX_INDEX_OBJECTS: usize = 5;
pub const SLOTS: usize = 256;
pub const SLOT_TABLE_BYTES: usize = SLOTS * 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GhostsFile {
    pub objects: Vec<Vec<u8>>,
}

impl GhostsFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let crc = r.u32()?;
        let computed = crc32_bzip2(&data[4..]);
        if crc != computed {
            return Err(Error::invalid(format!("ghosts: CRC {crc:#010x}, computed {computed:#010x}")));
        }
        let version = r.u32()?;
        if version != VERSION {
            return Err(Error::invalid(format!("ghosts: version {version:#x}, the game writes {VERSION:#x}")));
        }
        let count = r.u32()? as usize;
        let mut objects = Vec::with_capacity(count.min(MAX_INDEX_OBJECTS));
        for _ in 0..count {
            let n = r.u32()? as usize;
            objects.push(r.bytes(n)?.to_vec());
        }
        if data[r.pos()..].iter().any(|&b| b != 0) {
            return Err(Error::invalid(format!("ghosts: {} bytes after the objects", data.len() - r.pos())));
        }
        Ok(GhostsFile { objects })
    }

    // TODO(guess): the content file holds the whole buffer (0x3C00 for the index, 0x3E800 for a ghost: `Function_82506CC0`'s arguments), zero but for the objects, the CRC over the buffer after the first word; known: the serializer 0x82506EC0 writes the version 0xC at +4, the list (0x825582C0: a count, then each object length-prefixed) from +8 and the CRC with 0x82FE8810 over size − 4; settle: a console save.
    pub fn write(&self, buffer_size: usize) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.extend(VERSION.to_be_bytes());
        out.extend((self.objects.len() as u32).to_be_bytes());
        for o in &self.objects {
            out.extend((o.len() as u32).to_be_bytes());
            out.extend(o);
        }
        out.resize(out.len().max(buffer_size), 0);
        let crc = crc32_bzip2(&out[4..]);
        out[..4].copy_from_slice(&crc.to_be_bytes());
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhostIndex {
    pub slots: [u32; SLOTS],
    pub database: Vec<u8>,
}

impl Default for GhostIndex {
    fn default() -> Self {
        GhostIndex { slots: [0; SLOTS], database: Vec::new() }
    }
}

impl GhostIndex {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let mut slots = [0u32; SLOTS];
        for s in &mut slots {
            *s = r.u32()?;
        }
        let n = r.u32()? as usize;
        let database = r.bytes(n)?.to_vec();
        if r.remaining() != 0 {
            return Err(Error::invalid(format!("ghosts: index object has {} bytes left", r.remaining())));
        }
        Ok(GhostIndex { slots, database })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SLOT_TABLE_BYTES + 4 + self.database.len());
        for s in &self.slots {
            out.extend(s.to_be_bytes());
        }
        out.extend((self.database.len() as u32).to_be_bytes());
        out.extend(&self.database);
        out
    }

    pub fn find_slot(&self, track: u32) -> Option<usize> {
        if track == 0 {
            return None;
        }
        self.slots.iter().position(|&t| t == track)
    }

    pub fn allocate_slot(&mut self, track: u32) -> Option<usize> {
        let i = self.slots.iter().position(|&t| t == 0)?;
        self.slots[i] = track;
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_in_its_buffer() {
        let f = GhostsFile { objects: vec![vec![1, 2, 3], Vec::new(), vec![9; 10]] };
        let bytes = f.write(INDEX_BUFFER_SIZE);
        assert_eq!(bytes.len(), INDEX_BUFFER_SIZE);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0xC]);
        assert_eq!(&bytes[8..12], &[0, 0, 0, 3]);
        assert_eq!(GhostsFile::parse(&bytes).unwrap(), f);
        let mut bad = bytes.clone();
        bad[20] ^= 1;
        assert!(GhostsFile::parse(&bad).is_err());
        let big = vec![7u8; INDEX_BUFFER_SIZE * 2];
        let f = GhostsFile { objects: vec![big] };
        assert_eq!(GhostsFile::parse(&f.write(INDEX_BUFFER_SIZE)).unwrap(), f);
    }

    #[test]
    fn the_index_keeps_its_slot_table_and_database() {
        let mut i = GhostIndex::default();
        assert_eq!(i.allocate_slot(0x1234), Some(0));
        assert_eq!(i.allocate_slot(0x5678), Some(1));
        assert_eq!(i.find_slot(0x5678), Some(1));
        assert_eq!(i.find_slot(0), None);
        i.database = b"DB".to_vec();
        let bytes = i.write();
        assert_eq!(bytes.len(), SLOT_TABLE_BYTES + 4 + 2);
        assert_eq!(GhostIndex::parse(&bytes).unwrap(), i);
        assert!(GhostIndex::parse(&bytes[..bytes.len() - 1]).is_err());
        for k in 2..SLOTS {
            assert_eq!(i.allocate_slot(k as u32 + 1), Some(k));
        }
        assert_eq!(i.allocate_slot(999), None);
    }
}
