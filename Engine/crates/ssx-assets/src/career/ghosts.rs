use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ssx_formats::save::ghosts::{GHOST_BUFFER_SIZE, GhostIndex, GhostsFile, INDEX_BUFFER_SIZE, SLOTS};
use ssx_formats::t3db::Database;

use super::save::SaveSchema;

pub const COMMON_GROUP: &str = "SSXV_COMMON";
pub const GHOSTS_FILE: &str = "ssxvghosts";
pub const SCORES_TABLE: &str = "common_explore_scores";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScoreRow {
    pub score: u32,
    pub ghost_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhostStore {
    pub slots: [u32; SLOTS],
    pub rows: BTreeMap<u32, ScoreRow>,
}

impl Default for GhostStore {
    fn default() -> Self {
        GhostStore { slots: [0; SLOTS], rows: BTreeMap::new() }
    }
}

// TODO(guess): a port choice — the index buffer is the file `ssxvghosts` next to the profile and a slot's ghost buffer the file `ssxvghosts<slot>`, the ghost object the GCMP container of the image as a disc ghost is; known: the container is the console file `ssxvghosts` (0x82036930, enumerated as `ssxvghosts*` 0x8203693C), built by 0x8244DAE0 with the index buffer 0x3C00 and the ghost buffer 0x3E800, its serializer 0x82506EC0 writes the index objects in pass 0 and the ghost of slot +0x3A8 in pass 1 (the fetcher 0x8253FB78 case 4 reads it back by slot); settle: the EA save library's file names under 0x82FEE5E0 and whether the ghost object is the image or its container.
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join(GHOSTS_FILE)
}

pub fn ghost_path(dir: &Path, slot: usize) -> PathBuf {
    dir.join(format!("{GHOSTS_FILE}{slot}"))
}

impl GhostStore {
    pub fn load(dir: &Path, schema: &SaveSchema) -> Result<Self> {
        let path = index_path(dir);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&bytes, schema).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(bytes: &[u8], schema: &SaveSchema) -> Result<Self> {
        let file = GhostsFile::parse(bytes)?;
        let Some(first) = file.objects.first() else { return Ok(Self::default()) };
        let index = GhostIndex::parse(first)?;
        let db = Database::parse(&index.database)?;
        let short = schema.table_short(SCORES_TABLE)?;
        let t = db.table(&short).ok_or_else(|| anyhow!("the ghosts index has no table {SCORES_TABLE}"))?;
        let mut rows = BTreeMap::new();
        for r in t.live_rows() {
            let get = |field: &str| -> Result<u32> {
                t.get(r, &schema.short(SCORES_TABLE, field)?).ok_or_else(|| anyhow!("{SCORES_TABLE}.{field} row {r}"))
            };
            rows.insert(get("TRACKID")?, ScoreRow { score: get("SCORE")?, ghost_id: get("GHOSTID")? });
        }
        Ok(GhostStore { slots: index.slots, rows })
    }

    pub fn write(&self, schema: &SaveSchema) -> Result<Vec<u8>> {
        let mut db = schema.database.clone();
        let common: Vec<(String, u16)> = schema
            .meta
            .tables
            .iter()
            .filter(|t| t.save_groups.as_deref() == Some(COMMON_GROUP))
            .map(|t| (t.short.clone(), t.max_insert.unwrap_or(0).clamp(0, 0xFFFF) as u16))
            .collect();
        for t in &mut db.tables {
            if !common.iter().any(|(s, _)| *s == t.name_str()) {
                t.flags &= !ssx_formats::t3db::FLAG_WRITTEN;
            }
        }
        let short = schema.table_short(SCORES_TABLE)?;
        let cap = common.iter().find(|(s, _)| *s == short).map_or(0, |(_, c)| *c);
        let t = db.table_mut(&short).ok_or_else(|| anyhow!("ssxv.db has no {short}"))?;
        t.reserve(cap.max(self.rows.len() as u16));
        for (&track, row) in &self.rows {
            let i = t.insert_row()?;
            t.put(i, &schema.short(SCORES_TABLE, "TRACKID")?, track)?;
            t.put(i, &schema.short(SCORES_TABLE, "SCORE")?, row.score)?;
            t.put(i, &schema.short(SCORES_TABLE, "GHOSTID")?, row.ghost_id)?;
        }
        let index = GhostIndex { slots: self.slots, database: db.write() };
        Ok(GhostsFile { objects: vec![index.write()] }.write(INDEX_BUFFER_SIZE))
    }

    pub fn save(&self, dir: &Path, schema: &SaveSchema) -> Result<()> {
        let bytes = self.write(schema)?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = index_path(dir);
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    pub fn row(&self, track: u32) -> Option<ScoreRow> {
        self.rows.get(&track).copied()
    }

    pub fn slot_for(&mut self, track: u32) -> Result<usize> {
        let index = GhostIndex { slots: self.slots, database: Vec::new() };
        if let Some(i) = index.find_slot(track) {
            return Ok(i);
        }
        let mut index = index;
        let i = index.allocate_slot(track).ok_or_else(|| anyhow!("all {SLOTS} ghost slots are taken"))?;
        self.slots = index.slots;
        Ok(i)
    }

    pub fn upsert(&mut self, track: u32, score: u32, ghost_id: u32) {
        self.rows.insert(track, ScoreRow { score, ghost_id });
    }

    pub fn commit(&mut self, dir: &Path, schema: &SaveSchema, track: u32, score: u32, ghost: &[u8]) -> Result<usize> {
        if ghost.len() > GHOST_BUFFER_SIZE - 16 {
            bail!("the ghost ({} bytes) does not fit the slot's buffer ({GHOST_BUFFER_SIZE} bytes)", ghost.len());
        }
        let slot = self.slot_for(track)?;
        self.upsert(track, score, slot as u32);
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = ghost_path(dir, slot);
        let bytes = GhostsFile { objects: vec![ghost.to_vec()] }.write(GHOST_BUFFER_SIZE);
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        self.save(dir, schema)?;
        Ok(slot)
    }
}

#[derive(Clone, Debug)]
pub struct PersonalBest {
    pub slot: usize,
    pub score: u32,
    pub replay: crate::ghosts::GhostReplay,
}

// TODO(guess): a port choice — the personal-best replay's id is 0x8000_0000 + its slot, apart from the disc ghosts' ids (a ghost's id names its billboard and the scripts' lookups); known: the fetcher (0x8253FB78 case 4) hands the slot's ghost over as a leaderboard ghost by slot; settle: none.
pub const PERSONAL_BEST_ID: u32 = 0x8000_0000;

pub fn load_personal_best(dir: &Path, schema: &SaveSchema, track: u32) -> Result<Option<PersonalBest>> {
    let store = GhostStore::load(dir, schema)?;
    let Some(row) = store.row(track) else { return Ok(None) };
    let slot = row.ghost_id as usize;
    if slot >= SLOTS || store.slots[slot] != track {
        bail!("the ghosts index's row for drop {track:#010x} names slot {slot}, which is not its slot");
    }
    let bytes = read_ghost(dir, slot)?;
    let file = ssx_formats::ghost::GhostFile::from_container(&bytes)
        .with_context(|| format!("parsing {}", ghost_path(dir, slot).display()))?;
    let replay = crate::ghosts::GhostReplay::from_file(PERSONAL_BEST_ID + slot as u32, &file)?;
    Ok(Some(PersonalBest { slot, score: row.score, replay }))
}

pub fn read_ghost(dir: &Path, slot: usize) -> Result<Vec<u8>> {
    let path = ghost_path(dir, slot);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let file = GhostsFile::parse(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    file.objects.into_iter().next().ok_or_else(|| anyhow!("{} holds no ghost", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::save::tests::schema;

    #[test]
    fn the_store_round_trips_through_its_buffer() {
        let s = schema();
        let mut g = GhostStore::default();
        assert_eq!(g.slot_for(77).unwrap(), 0);
        assert_eq!(g.slot_for(78).unwrap(), 1);
        assert_eq!(g.slot_for(77).unwrap(), 0);
        g.upsert(77, 92_631, 0);
        g.upsert(78, 120_000, 1);
        g.upsert(77, 90_000, 0);
        let bytes = g.write(&s).unwrap();
        assert_eq!(bytes.len(), INDEX_BUFFER_SIZE);
        let back = GhostStore::parse(&bytes, &s).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.row(77), Some(ScoreRow { score: 90_000, ghost_id: 0 }));
        assert_eq!(back.row(79), None);
        let db = GhostIndex::parse(&GhostsFile::parse(&bytes).unwrap().objects[0]).unwrap().database;
        let db = Database::parse(&db).unwrap();
        assert!(db.table("tggg").is_some() && db.table("tfff").is_none());
    }

    #[test]
    fn slots_run_out_at_256() {
        let mut g = GhostStore::default();
        for k in 1..=SLOTS as u32 {
            assert_eq!(g.slot_for(k).unwrap(), k as usize - 1);
        }
        assert!(g.slot_for(1000).is_err());
        assert_eq!(g.slot_for(3).unwrap(), 2);
    }

    #[test]
    fn a_commit_writes_the_slot_and_the_index() {
        let s = schema();
        let dir = std::env::temp_dir().join(format!("pa-ghosts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut g = GhostStore::load(&dir, &s).unwrap();
        assert_eq!(g, GhostStore::default());
        let slot = g.commit(&dir, &s, 77, 92_631, b"GHOST").unwrap();
        assert_eq!(slot, 0);
        assert_eq!(read_ghost(&dir, 0).unwrap(), b"GHOST");
        assert_eq!(std::fs::metadata(ghost_path(&dir, 0)).unwrap().len(), GHOST_BUFFER_SIZE as u64);
        let again = GhostStore::load(&dir, &s).unwrap();
        assert_eq!(again.row(77), Some(ScoreRow { score: 92_631, ghost_id: 0 }));
        let mut again = again;
        assert_eq!(again.commit(&dir, &s, 77, 90_000, b"BETTER").unwrap(), 0);
        assert_eq!(read_ghost(&dir, 0).unwrap(), b"BETTER");
        assert!(g.commit(&dir, &s, 78, 1, &vec![0u8; GHOST_BUFFER_SIZE]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_personal_best_loads_back_as_a_replay() {
        use ssx_formats::ghost::{BUILD, GhostRecorder, POSE_BLOCK_SIZE, PoseBlock, UserHeader};
        let s = schema();
        let dir = std::env::temp_dir().join(format!("pa-pb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_personal_best(&dir, &s, 77).unwrap().is_none());
        let mut r = GhostRecorder::with_interval(0.25);
        for i in 0..5 {
            let bytes: [u8; POSE_BLOCK_SIZE] = std::array::from_fn(|k| (k as u8).wrapping_mul(3).wrapping_add(i as u8));
            r.sample(i as f32 * 0.25, [i as f32, 0.0, 1.0], 10.0 * i as f32, PoseBlock::parse(&bytes).unwrap());
        }
        let header = UserHeader { gamertag: "Rider".into(), gear_flags: 2, score: 92_631, build: BUILD };
        let ghost = ssx_formats::ghost::compress(&r.to_file(header).write());
        let mut g = GhostStore::default();
        g.commit(&dir, &s, 78, 1, b"OTHER").unwrap();
        assert_eq!(g.commit(&dir, &s, 77, 92_631, &ghost).unwrap(), 1);
        let pb = load_personal_best(&dir, &s, 77).unwrap().unwrap();
        assert_eq!((pb.slot, pb.score, pb.replay.id), (1, 92_631, PERSONAL_BEST_ID + 1));
        assert_eq!(pb.replay.header.gamertag, "Rider");
        assert_eq!(pb.replay.times.len(), 5);
        assert_eq!(pb.replay.position(0.5), [2.0, 0.0, 1.0]);
        assert!(load_personal_best(&dir, &s, 78).is_err());
        assert!(load_personal_best(&dir, &s, 79).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
