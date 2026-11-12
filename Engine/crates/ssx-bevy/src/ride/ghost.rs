use anyhow::Result;
use ssx_assets::rig_anim::RigPose;
use ssx_formats::ghost::{BUILD, GhostRecorder, PoseBlock, UserHeader};
use ssx_physics::graph::{LandingRumble, RiderGraph, pose_source};
use ssx_physics::rider::RiderState;

#[derive(Default)]
pub struct GhostRecording {
    rumble: LandingRumble,
    pub recorder: GhostRecorder,
    pub failures: u64,
}

impl GhostRecording {
    pub fn new() -> Self {
        GhostRecording { recorder: GhostRecorder::new(), ..Default::default() }
    }

    pub fn set_interval(&mut self, interval: f32) {
        if self.recorder.is_empty() && interval > 0.0 {
            self.recorder.interval = interval;
        }
    }

    pub fn reset(&mut self) {
        self.rumble = LandingRumble::default();
        self.recorder.clear();
    }

    // TODO(guess): the POSI sample is the trajectory position and the SCOR sample the running score, taken on the event clock; known: the reader's `position` drives the ghost's billboard and `fed_score` a bot's points, the disc ghosts' POSI and GMPY trajectory positions agree; settle: the three stream writers under `GhostRecorder_Init` 0x824BE0D8.
    pub fn step(&mut self, time: f32, s: &RiderState, g: &RiderGraph, posed: &RigPose) -> Result<()> {
        let landing = self.rumble.step(s);
        if !self.recorder.due(time) {
            return Ok(());
        }
        let block = match pose_source(s, g, posed, landing).map(|src| PoseBlock::record(&src)) {
            Ok(b) => b,
            Err(e) => {
                self.failures += 1;
                return Err(e);
            }
        };
        self.recorder.sample(time, s.traj.to_array(), s.score.score, block);
        Ok(())
    }

    pub fn container(&self, gamertag: &str, gear_flags: u16, score: u32) -> Vec<u8> {
        let header = UserHeader { gamertag: gamertag.to_string(), gear_flags, score, build: BUILD };
        ssx_formats::ghost::compress(&self.recorder.to_file(header).write())
    }
}

// TODO(guess): the header's gear flags as bit (type − 1) of each equipped gear type (the ice axe at bit 1, which the ghost rider's prop reads as `gear_flags & 2`); known: the drop's slot table `survival.slots` is indexed by gear type (`SLOT_ICE_AXE`); settle: the gear word written at user header +0x32 under `GhostFile_WriteHeader` 0x8253DC98's caller.
pub fn gear_flags(slots: &[bool]) -> u16 {
    slots.iter().enumerate().skip(1).fold(0u16, |acc, (i, &on)| acc | (u16::from(on) << (i - 1)))
}

pub struct PendingGhost {
    pub track: u32,
    pub score: u32,
    pub container: Vec<u8>,
    pub samples: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gear_flags_put_the_ice_axe_at_bit_one() {
        let mut slots = [false; 15];
        slots[ssx_physics::character::SLOT_ICE_AXE] = true;
        assert_eq!(gear_flags(&slots), 2);
        slots[0] = true;
        assert_eq!(gear_flags(&slots), 2);
        slots[1] = true;
        assert_eq!(gear_flags(&slots), 3);
    }
}
