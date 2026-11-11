use glam::Vec3;

use crate::ballistic::BallisticRay;
use crate::collision::{Hit, RIDABLE_CLASS_MASK, SegmentQuery};
use crate::rider::{World, drag_accel};
use crate::surface::Weights;

pub const GROUND_PROBE_LENGTH: f32 = 2.0;
pub const GROUND_PROBE_LENGTH_SHORT: f32 = 0.5;
pub const VELOCITY_RAY_LENGTH_GROUND: f32 = 6.0;
pub const VELOCITY_RAY_LENGTH_AIR: f32 = 0.1;
pub const HELPER_RAY_LENGTH: f32 = 30.0;
pub const REFINE_SWEEP_RADIUS: f32 = 0.4;
pub const BALLISTIC_FALLBACK_DISTANCE: f32 = 2.5;
pub const CLOSE_TO_GROUND: f32 = 1.4;
pub const VERY_CLOSE_TO_GROUND: f32 = 1.3;

#[derive(Clone, Copy, Debug)]
pub struct GroundContact {
    pub point: Vec3,
    pub normal: Vec3,
    pub face_normal: Vec3,
    pub contact: Vec3,
    pub height: f32,
    pub distance: f32,
    pub weights: Weights,
    pub surface_id: u32,
    pub class: u8,
    pub ridable_pass: bool,
    pub entity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloseToGround {
    pub d: f32,
    pub is_close: bool,
    pub is_very_close: bool,
}

impl Default for CloseToGround {
    fn default() -> Self {
        Self { d: f32::MAX, is_close: false, is_very_close: false }
    }
}

pub fn close_to_ground(contact: Option<&GroundContact>, snow_depth: f32) -> CloseToGround {
    match contact {
        Some(c) => close_at(c.distance, snow_depth),
        None => CloseToGround::default(),
    }
}

pub fn close_at(distance: f32, snow_depth: f32) -> CloseToGround {
    let d = distance - snow_depth;
    CloseToGround { d, is_close: d <= CLOSE_TO_GROUND, is_very_close: d <= VERY_CLOSE_TO_GROUND }
}

pub fn use_unfiltered(a: Option<Vec3>, b: Option<Vec3>, w: Vec3, cos_horizontal: f32) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(a), Some(b)) => a.y > cos_horizontal && a.dot(b) > cos_horizontal,
        (Some(a), None) => a.y > cos_horizontal && a.dot(w) > cos_horizontal,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProbeInputs {
    pub traj: Vec3,
    pub joint: Vec3,
    pub prev_normal: Vec3,
    pub velocity: Vec3,
    pub in_air: bool,
    pub short: bool,
    pub grinding: bool,
    pub uber_in_air: bool,
    pub wind: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct PassResult {
    pub hit: Option<Hit>,
    pub helper_normal: Option<Vec3>,
}

impl World {
    fn probe_query(&self, start: Vec3, end: Vec3, radius: f32, two_sided: bool, mask: u32) -> Option<Hit> {
        self.collision.segment_query(&SegmentQuery { start, end, radius, two_sided, class_mask: mask })
    }

    pub fn probe_pass(&self, i: &ProbeInputs, mask: u32, aux: Option<&BallisticRay>) -> PassResult {
        let (t, j) = (i.traj, i.joint);
        let len = if i.short { GROUND_PROBE_LENGTH_SHORT } else { GROUND_PROBE_LENGTH };
        let speed = i.velocity.length();
        let v_hat = if speed > 1.525_878_9e-5 { i.velocity / speed } else { Vec3::X };
        let lv = if i.in_air { VELOCITY_RAY_LENGTH_AIR } else { VELOCITY_RAY_LENGTH_GROUND };
        let to_joint = (j - t).try_normalize().unwrap_or(Vec3::ZERO);
        let ends = [
            (t, t + to_joint * len),
            (j, j - i.prev_normal * len),
            (j, j - Vec3::Y * len),
            (t, t - Vec3::Y * len),
            (t, t + v_hat * lv),
        ];
        let fat = self.tuning.queries.fat_probe_ray_radius;
        let r = &self.props;
        let (kh, kv, g) = if i.in_air {
            (r.k_h_air, r.k_v_air, r.g_air.y.abs())
        } else {
            (r.k_h_ground, r.k_v_ground, r.g_ground.y.abs())
        };
        let tl = self.tuning.queries.ballistic_probe_lookahead_ticks;
        let mut a = drag_accel(kh, kv, i.velocity, i.wind, tl);
        a.y -= g;
        let b_end = t + (i.velocity + a * tl) * tl;
        let chord = self.probe_query(t, b_end, self.tuning.queries.ballistic_probe_ray_radius, false, mask);
        let helper = self.probe_query(t, t - Vec3::Y * HELPER_RAY_LENGTH, 0.0, false, mask);
        let mut hit = ends
            .iter()
            .find_map(|&(s, e)| self.probe_query(s, e, 0.0, false, mask))
            .or_else(|| ends.iter().find_map(|&(s, e)| self.probe_query(s, e, fat, false, mask)))
            .or(chord);
        if hit.is_none()
            && i.in_air
            && let Some(b) = aux.filter(|b| b.valid)
            && let Some(h) = b.hit
            && (h.point - t).length_squared() < BALLISTIC_FALLBACK_DISTANCE * BALLISTIC_FALLBACK_DISTANCE
        {
            hit = Some(h);
        }
        if let Some(h) = hit
            && let Some(m) = self.probe_query(t, h.point, REFINE_SWEEP_RADIUS, true, mask)
        {
            hit = Some(m);
        }
        if let Some(h) = hit
            && !i.grinding
            && chord.is_none()
            && (t - h.point).dot(h.normal) > 0.0
            && (b_end - h.point).dot(h.normal) < 0.0
        {
            hit = None;
        }
        PassResult { hit, helper_normal: helper.map(|h| h.normal) }
    }

    pub fn ground_probe(&self, i: &ProbeInputs, ballistic: &BallisticRay) -> Option<GroundContact> {
        let a = self.probe_pass(i, 0, Some(ballistic));
        let b = self.probe_pass(i, RIDABLE_CLASS_MASK, None);
        let cos = self.tuning.queries.cos_surface_is_horizontal_angle_threshold;
        let w = b.helper_normal.unwrap_or(Vec3::Y);
        let use_a = i.uber_in_air || use_unfiltered(a.hit.map(|h| h.normal), b.hit.map(|h| h.normal), w, cos);
        let (hit, ridable_pass) = if use_a { (a.hit, false) } else { (b.hit, true) };
        let h = hit?;
        let n = h.normal;
        let c = &self.collision;
        let cluster = &c.clusters[h.cluster as usize];
        Some(GroundContact {
            point: h.point,
            normal: n,
            face_normal: h.contact_normal,
            contact: i.joint + n * (h.point - i.joint).dot(n),
            height: (i.joint - h.point).dot(n),
            distance: (i.traj - h.point).dot(n),
            weights: c.surface_weights(h.cluster, h.tri, h.bary, n.y),
            surface_id: cluster.surface_id,
            class: cluster.class,
            ridable_pass,
            entity: c.cluster_entity[h.cluster as usize],
        })
    }
}

#[cfg(test)]
mod tests;
