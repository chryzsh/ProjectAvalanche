pub mod chunks;
pub mod ghosts;

use crate::reader::Reader;
use crate::{Error, Result};

pub const PROFILE_BUFFER_SIZE: usize = 0x57800;

pub const PROFILE_CHUNKS: usize = 11;

pub fn crc32_bzip2(data: &[u8]) -> u32 {
    !crate::t3db::crc32(data, 0xFFFF_FFFF)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkInfo {
    pub name: &'static str,
    pub owner: &'static str,
    pub save: u32,
    pub load: u32,
}

pub const CHUNKS: [ChunkInfo; PROFILE_CHUNKS] = [
    ChunkInfo {
        name: "database",
        owner: "ProfileDb +0xF68 (T3DB group SSXV_PROFILE)",
        save: 0x824A_76C0,
        load: 0x824A_7780,
    },
    ChunkInfo { name: "online", owner: "online manager [0x838AB460] +4", save: 0x8289_BD90, load: 0x8289_BE18 },
    ChunkInfo { name: "flow", owner: "mpFlowManager +0x1FD4", save: 0x8246_7E98, load: 0x8246_7F78 },
    ChunkInfo { name: "stats", owner: "mStatsDataManager +0x1FDC", save: 0x824B_4A28, load: 0x824B_4AB8 },
    ChunkInfo { name: "badges", owner: "mBadgeManager +0xF64", save: 0x8248_87F8, load: 0x8248_88B0 },
    ChunkInfo { name: "geotags", owner: "geotag manager +0xF98 (+4)", save: 0x8243_FFD8, load: 0x8243_FFE0 },
    ChunkInfo { name: "friends", owner: "friend challenges +0xF94 (+4)", save: 0x8246_A330, load: 0x8246_B398 },
    ChunkInfo { name: "tuning", owner: "TuningCache +0xFA0", save: 0x824B_4CA0, load: 0x824B_4D10 },
    ChunkInfo { name: "activity", owner: "ActivityManager +0xF5C", save: 0x8245_2328, load: 0x8245_2C90 },
    ChunkInfo { name: "front end", owner: "mFESaveData +0xFA8", save: 0x824B_2AD0, load: 0x824B_2C48 },
    ChunkInfo { name: "key set", owner: "[0x838AB910] +0x14C", save: 0x823E_2338, load: 0x823E_23E8 },
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileFile {
    pub chunks: Vec<Vec<u8>>,
    pub tail: Vec<u8>,
}

impl ProfileFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let crc = r.u32()?;
        let computed = crc32_bzip2(&data[4..]);
        if crc != computed {
            return Err(Error::invalid(format!("save: CRC {crc:#010x}, computed {computed:#010x}")));
        }
        let count = r.u32()? as usize;
        if count != PROFILE_CHUNKS {
            return Err(Error::invalid(format!("save: {count} chunks, the game registers {PROFILE_CHUNKS}")));
        }
        let mut chunks = Vec::with_capacity(count);
        for _ in 0..count {
            let n = r.u32()? as usize;
            chunks.push(r.bytes(n)?.to_vec());
        }
        let tail = data[r.pos()..].to_vec();
        Ok(ProfileFile { chunks, tail })
    }

    fn body(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend((self.chunks.len() as u32).to_be_bytes());
        for c in &self.chunks {
            b.extend((c.len() as u32).to_be_bytes());
            b.extend(c);
        }
        b
    }

    pub fn used_len(&self) -> usize {
        4 + self.body().len()
    }

    // TODO(guess): the content file holds the whole buffer (the CRC covers buffer size − 4 bytes, 0x82506B50) and its unused part is zero but for the key set's three bytes; settle: EA's save-library write path below 0x82506B18 and a console save.
    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.extend(self.body());
        out.extend(&self.tail);
        out.resize(out.len().max(PROFILE_BUFFER_SIZE), 0);
        let crc = crc32_bzip2(&out[4..]);
        out[..4].copy_from_slice(&crc.to_be_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_bzip2() {
        assert_eq!(crc32_bzip2(b"123456789"), 0xFC89_1918);
    }

    #[test]
    fn buffers_round_trip_and_check_their_crc() {
        let f = ProfileFile { chunks: (0..11).map(|i| vec![i as u8; i]).collect(), tail: Vec::new() };
        let bytes = f.write();
        assert_eq!(bytes.len(), PROFILE_BUFFER_SIZE);
        let back = ProfileFile::parse(&bytes).unwrap();
        assert_eq!(back.chunks, f.chunks);
        assert_eq!(back.write(), bytes);
        let mut bad = bytes.clone();
        bad[100] ^= 1;
        assert!(ProfileFile::parse(&bad).is_err());
        let short = ProfileFile { chunks: vec![vec![]; 3], tail: Vec::new() }.write();
        assert!(ProfileFile::parse(&short).is_err());
    }
}
