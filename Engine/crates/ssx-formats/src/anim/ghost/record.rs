use super::{GMPY, GhostFile, POSE_BLOCK_SIZE, POSI, PoseBlock, SCOR, Stream, UserHeader, VERSION};

// TODO(guess): the sample interval 0.2 s; known: `GhostRecorder_Init` 0x824BE0D8 reads it from tunable 0xCF (0x8236DD70's table, vtable +0x14) and the disc ghosts carry it in each stream descriptor (`Stream::interval`), which `GhostRecorder::with_interval` takes when a disc ghost of the drop is loaded; settle: tunable 0xCF's value.
pub const DEFAULT_INTERVAL: f32 = 0.2;

// TODO(guess): the streams' alignments (16 for the pose blocks, 4 for the rest); known: the reader checks the data offset against the descriptor's alignment and the disc ghosts pass; settle: the three 0x824B6970 calls' stream descriptions (0x837EBCFC, 0x837EBCEC, 0x837EBCE8).
const POSE_ALIGNMENT: u32 = 16;
const WORD_ALIGNMENT: u32 = 4;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GhostRecorder {
    pub interval: f32,
    pub times: Vec<f32>,
    pub positions: Vec<[f32; 3]>,
    pub scores: Vec<f32>,
    pub poses: Vec<PoseBlock>,
}

impl GhostRecorder {
    pub fn new() -> Self {
        Self::with_interval(DEFAULT_INTERVAL)
    }

    pub fn with_interval(interval: f32) -> Self {
        GhostRecorder { interval: if interval > 0.0 { interval } else { DEFAULT_INTERVAL }, ..Default::default() }
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn clear(&mut self) {
        self.times.clear();
        self.positions.clear();
        self.scores.clear();
        self.poses.clear();
    }

    pub fn due(&self, time: f32) -> bool {
        match self.times.last() {
            None => true,
            Some(&last) => time < last || time - last >= self.interval,
        }
    }

    // TODO(guess): a sample at a time at or before the last one (a rewind) drops the samples from that time on; known: the reader rejects a sample whose time is not past the previous one (`GhostReplay::from_file`); settle: what `GhostRecorder_Init` 0x824BE0D8's recorder does with the stream on a rewind.
    pub fn sample(&mut self, time: f32, position: [f32; 3], score: f32, pose: PoseBlock) {
        let keep = self.times.partition_point(|&t| t < time);
        self.times.truncate(keep);
        self.positions.truncate(keep);
        self.scores.truncate(keep);
        self.poses.truncate(keep);
        self.times.push(time);
        self.positions.push(position);
        self.scores.push(score);
        self.poses.push(pose);
    }

    pub fn to_file(&self, header: UserHeader) -> GhostFile {
        let n = self.times.len();
        let mut scores = Vec::with_capacity(n * 4);
        for s in &self.scores {
            scores.extend_from_slice(&s.to_be_bytes());
        }
        let mut positions = Vec::with_capacity(n * 12);
        for p in &self.positions {
            for c in p {
                positions.extend_from_slice(&c.to_be_bytes());
            }
        }
        let mut poses = Vec::with_capacity(n * POSE_BLOCK_SIZE);
        for b in &self.poses {
            poses.extend_from_slice(&b.to_bytes());
        }
        let streams = vec![
            Stream::new(SCOR, 4, WORD_ALIGNMENT, self.interval, self.times.clone(), scores),
            Stream::new(POSI, 12, WORD_ALIGNMENT, self.interval, self.times.clone(), positions),
            Stream::new(GMPY, POSE_BLOCK_SIZE as u32, POSE_ALIGNMENT, self.interval, self.times.clone(), poses),
        ];
        GhostFile { crc: 0, version: VERSION, unk_1a: 0, header, streams }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BUILD, GAMERTAG_BYTES, GhostFile, compress};
    use super::*;

    fn pose(i: usize) -> PoseBlock {
        let bytes: [u8; POSE_BLOCK_SIZE] = std::array::from_fn(|k| (k as u8).wrapping_mul(7).wrapping_add(i as u8));
        PoseBlock::parse(&bytes).unwrap()
    }

    fn header() -> UserHeader {
        UserHeader { gamertag: "Rider".into(), gear_flags: 2, score: 92_631, build: BUILD }
    }

    #[test]
    fn a_recorded_ride_round_trips_through_the_reader() {
        let mut r = GhostRecorder::with_interval(0.25);
        for i in 0..7 {
            let t = i as f32 * 0.25;
            assert!(r.due(t));
            r.sample(t, [t, -t * 2.0, 3.0], 100.0 * t, pose(i));
            assert!(!r.due(t + 0.1));
        }
        let file = r.to_file(header());
        let image = file.write();
        let back = GhostFile::parse(&image).unwrap();
        assert_eq!(back.header, header());
        assert_eq!(back.version, VERSION);
        assert_eq!(back.positions().unwrap(), r.positions);
        assert_eq!(back.scores().unwrap(), r.scores);
        assert_eq!(back.poses().unwrap(), r.poses);
        for s in &back.streams {
            assert_eq!(s.times, r.times);
            assert_eq!(s.interval, 0.25);
            assert_eq!((s.count, s.capacity), (7, 7));
        }
        let container = compress(&image);
        assert_eq!(GhostFile::from_container(&container).unwrap(), back);
        assert_eq!(&image[4..8], b"GHST");
        assert_eq!(u32::from_be_bytes(image[8..12].try_into().unwrap()), 0x58);
        assert_eq!(u32::from_be_bytes(image[12..16].try_into().unwrap()), 3);
        assert_eq!(u16::from_be_bytes(image[24..26].try_into().unwrap()), 0x1A);
        assert_eq!(&image[0x1C..0x21], b"Rider");
        assert_eq!(u32::from_be_bytes(image[0x54..0x58].try_into().unwrap()), BUILD);
    }

    #[test]
    fn a_rewind_drops_the_samples_from_that_time_on() {
        let mut r = GhostRecorder::new();
        for i in 0..5 {
            r.sample(i as f32, [0.0; 3], 0.0, pose(i));
        }
        r.sample(2.5, [1.0; 3], 9.0, pose(9));
        assert_eq!(r.times, [0.0, 1.0, 2.0, 2.5]);
        assert_eq!(r.scores, [0.0, 0.0, 0.0, 9.0]);
        r.sample(2.5, [2.0; 3], 10.0, pose(10));
        assert_eq!(r.times, [0.0, 1.0, 2.0, 2.5]);
        assert_eq!(r.positions[3], [2.0; 3]);
        r.clear();
        assert!(r.is_empty() && r.due(0.0));
    }

    #[test]
    fn long_gamertags_are_cut_to_the_field() {
        let mut r = GhostRecorder::new();
        r.sample(0.0, [0.0; 3], 0.0, pose(0));
        let long = "x".repeat(80);
        let file = r.to_file(UserHeader { gamertag: long, gear_flags: 0, score: 0, build: 0 });
        let back = GhostFile::parse(&file.write()).unwrap();
        assert_eq!(back.header.gamertag.len(), GAMERTAG_BYTES - 1);
    }
}
