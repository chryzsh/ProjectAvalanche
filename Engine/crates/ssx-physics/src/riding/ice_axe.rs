use anyhow::Result;

use crate::curve::Curve;
use crate::tags::{BoolTag, FloatTag, TagEvent, TagTracker, TagWindow};
use crate::tuning::AssetView;

#[derive(Clone, Debug, Default)]
pub struct IceAxeTuning {
    pub max_lean_angle: f32,
    pub equilibrium_lean_angle: f32,
    pub lean_activation_threshold: f32,
    pub blend_activation_threshold: f32,
    pub max_lean_input_curve: Curve,
    pub lean_input_transform: Curve,
    pub chop_lean_threshold: f32,
    pub steering_transform: Curve,
    pub steering_transform_full: Curve,
    pub steering_transform_blend: Curve,
    pub steering_filter_gain: f32,
    pub surface_weight_threshold: f32,
    pub noise_angle_normalization: f32,
}

impl IceAxeTuning {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            max_lean_angle: a.f32("MaxLeanAngle")?,
            equilibrium_lean_angle: a.f32("EquilibriumLeanAngle")?,
            lean_activation_threshold: a.f32("IceAxeSurfacePropertyLeanActivationThreshold")?,
            blend_activation_threshold: a.f32("IceAxeSurfacePropertyBlendActivationThreshold")?,
            max_lean_input_curve: a.curve("SpeedVsMaxLeanInputAngleCurveAsset")?,
            lean_input_transform: a.curve("LeanInputAngleTransform")?,
            chop_lean_threshold: a.f32("IceAxeChopLeanThreshold")?,
            steering_transform: a.curve("AxeSteeringInputAngleTransform")?,
            steering_transform_full: a.curve("AxeSteeringInputAngleTransformFull")?,
            steering_transform_blend: a.curve("AxeSteeringInputAngleTransformBlendCurve")?,
            steering_filter_gain: a.f32("IceAxeSteeringInputFilterGain")?,
            surface_weight_threshold: a.f32("IceAxeSurfaceWeightThreshold")?,
            noise_angle_normalization: a.f32("IceAxeNoiseAngleNormalizationValue")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IceAxeFrame {
    pub blend: f32,
    pub effectiveness: f32,
    pub active: bool,
    pub steering_value: f32,
    pub dig_modifier: f32,
}

pub const DEPLOY_SPLINE: [f32; 11] = [
    0.0,
    0.072_723_05,
    0.172_361_37,
    0.278_821_6,
    0.389_489_53,
    0.5,
    0.610_510_47,
    0.721_178_5,
    0.827_638_6,
    0.927_276_97,
    1.0,
];

pub const LAYER_SPLINE: [f32; 16] = [
    0.0,
    0.042_968_75,
    0.105_001_95,
    0.172_361_37,
    0.243_093_28,
    0.316_406_25,
    0.389_489_53,
    0.462_531_27,
    0.537_468_7,
    0.610_510_47,
    0.683_593_75,
    0.756_906_6,
    0.827_638_6,
    0.894_998,
    0.957_031_25,
    1.0,
];

pub fn tag_spline(s: &[f32], w: f32) -> f32 {
    let Some(&last) = s.last() else { return 0.0 };
    let x = (s.len() - 1) as f32 * w;
    let i = x as usize;
    if i + 1 >= s.len() {
        return last;
    }
    let f = x - i as f32;
    (1.0 - f) * s[i] + s[i + 1] * f
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DeployNode {
    #[default]
    Holstered,
    Deploy(f32),
    Deployed,
}

pub const DEPLOY_END_TICKS: f32 = 57.0;
const DEPLOY_WINDOWS: [TagWindow; 3] =
    [TagWindow::new(0.0, 15.0, false), TagWindow::new(30.0, 10.0, false), TagWindow::new(40.0, 10.0, false)];

pub const CHOP_END_TICKS: f32 = 29.0;
const CHOP_WINDOWS: [TagWindow; 3] =
    [TagWindow::new(6.0, 0.0, false), TagWindow::new(26.0, 0.0, false), TagWindow::new(26.0, 0.0, false)];
const CHOPPED_TAG: BoolTag = BoolTag { value: true, mode: 1, reset_on_leave: false };
// TODO(guess): the chop clip's weight tags (tick 26, ResetOnLeave) keep the arm down once they fire; known: the graph
// (layer_pose_riding → sf_ice_axe_chop_layer_*) writes FLT_ice_axe_chop_weight_* only there, on neutral→chop (1.0)
// and on chop→neutral (0.0), so chopped→neutral keeps the value and the original can only reach it at 0.0, while a
// restore on leave gives the stuck arm and drain of #60; settle: decode FloatGameStateTag's leave for zero-length windows
const CHOP_RESET_TAG: FloatTag = FloatTag { value: 0.0, mode: 1, reset_on_leave: false, spline: 1.0 };

#[derive(Clone, Debug, PartialEq)]
pub struct ChopLayer {
    pub chop: Option<f32>,
    pub chopped_node: bool,
    tags: TagTracker,
    exiting: bool,
    saved: (bool, f32, f32),
}

impl Default for ChopLayer {
    fn default() -> Self {
        Self {
            chop: None,
            chopped_node: false,
            tags: TagTracker::new(&CHOP_WINDOWS),
            exiting: false,
            saved: (false, 0.0, 0.0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IceAxeState {
    pub enabled: bool,
    pub deploy: DeployNode,
    deploy_tags: Option<TagTracker>,
    deploy_saved: [f32; 3],
    pub deployed: bool,
    pub blend_weight: f32,
    pub layer_weight: f32,
    pub noise: [f32; 2],
    pub steering_value: f32,
    pub effects_timer: f32,
    pub chopped: bool,
    pub start_chop: [bool; 2],
    pub chop_weight: [f32; 2],
    pub layers: [ChopLayer; 2],
    pub frame: IceAxeFrame,
}

impl IceAxeState {
    pub fn enter_gameplay(&mut self) {
        (self.deploy, self.deploy_tags, self.deploy_saved) = (DeployNode::Holstered, None, [0.0; 3]);
        self.layers = Default::default();
    }

    pub fn blend(&self, t: &IceAxeTuning, ice_weight: f32) -> f32 {
        let on_ice = if ice_weight > t.surface_weight_threshold { 1.0 } else { 0.0 };
        self.blend_weight * on_ice
    }

    #[allow(clippy::too_many_arguments)]
    pub fn read_inputs(
        &mut self,
        t: &IceAxeTuning,
        effectiveness: f32,
        ice_weight: f32,
        lean_prev: f32,
        stick_angle: f32,
        dig_modifier: f32,
        dt: f32,
    ) -> IceAxeFrame {
        let blend = self.blend(t, ice_weight);
        let active =
            blend >= t.blend_activation_threshold && lean_prev.abs() >= t.lean_activation_threshold && self.chopped;
        let a = stick_angle.abs();
        let lo = t.steering_transform.eval(a);
        let target =
            lo + (t.steering_transform_full.eval(a) - lo) * t.steering_transform_blend.eval(self.effects_timer);
        self.effects_timer = if active { self.effects_timer + dt } else { 0.0 };
        let e = (-(t.steering_filter_gain * dt)).exp();
        self.steering_value = e * (self.steering_value - target) + target;
        let f = self.steering_value.abs() / t.noise_angle_normalization;
        let r = if -f >= 0.0 { 0.0 } else { f };
        let r = if 1.0 - r >= 0.0 { r } else { 1.0 };
        let targets = [if lean_prev >= 0.0 { 0.0 } else { r }, if lean_prev > 0.0 { r } else { 0.0 }];
        for (n, target) in self.noise.iter_mut().zip(targets) {
            *n = e * (*n - target) + target;
        }
        self.frame = IceAxeFrame { blend, effectiveness, active, steering_value: self.steering_value, dig_modifier };
        self.frame
    }

    pub fn chop_update(&mut self, t: &IceAxeTuning, lean: f32) {
        if lean.abs() < t.lean_activation_threshold {
            self.chopped = false;
        }
        let chop = lean.abs() > t.chop_lean_threshold && (self.frame.blend - 1.0).abs() <= 1.525_878_9e-5;
        self.start_chop = [lean < 0.0 && chop, lean > 0.0 && chop];
    }

    pub fn layers_update(&mut self, enabled: bool, dt: f32) {
        self.enabled = enabled;
        match self.deploy {
            DeployNode::Holstered if enabled => {
                self.deploy = DeployNode::Deploy(dt);
                self.deploy_tags = Some(TagTracker::new(&DEPLOY_WINDOWS));
            }
            DeployNode::Deploy(t) if t + dt >= DEPLOY_END_TICKS => {
                self.deploy = DeployNode::Deployed;
                self.deployed = true;
            }
            DeployNode::Deploy(t) => self.deploy = DeployNode::Deploy(t + dt),
            _ => {}
        }
        if let Some(tags) = self.deploy_tags.as_mut() {
            let events = match self.deploy {
                DeployNode::Deploy(t) if tags.update(t) => tags.process(),
                DeployNode::Deploy(_) => Vec::new(),
                _ => tags.exit(),
            };
            for (i, e) in events {
                let w = match e {
                    TagEvent::Apply(w) | TagEvent::Leave(w) => w,
                    TagEvent::Enter => 0.0,
                };
                let (value, spline, var) = match i {
                    0 => (1.0, tag_spline(&LAYER_SPLINE, w), &mut self.layer_weight),
                    1 => (1.0, tag_spline(&DEPLOY_SPLINE, w), &mut self.blend_weight),
                    _ => (0.0, tag_spline(&DEPLOY_SPLINE, w), &mut self.layer_weight),
                };
                FloatTag { value, mode: 3, reset_on_leave: false, spline }.on(e, var, &mut self.deploy_saved[i]);
            }
            if self.deploy == DeployNode::Deployed {
                self.deploy_tags = None;
            }
        }
        for side in 0..2 {
            self.chop_layer(side, dt);
        }
    }

    fn chop_layer(&mut self, side: usize, dt: f32) {
        let trigger = self.start_chop[side];
        let l = &mut self.layers[side];
        let (mut weights, mut chopped) = (self.chop_weight, self.chopped);
        let mut transition_weight = None;
        match (l.chop, l.chopped_node) {
            (None, false) if trigger => {
                l.chop = Some(dt);
                l.tags = TagTracker::new(&CHOP_WINDOWS);
                transition_weight = Some(1.0);
            }
            (Some(_), _) if !trigger => {
                l.chop = None;
                l.exiting = true;
                transition_weight = Some(0.0);
            }
            (Some(t), _) if t + dt >= CHOP_END_TICKS => {
                (l.chop, l.chopped_node, l.exiting) = (None, true, true);
            }
            (Some(t), _) => l.chop = Some(t + dt),
            (None, true) if !trigger => l.chopped_node = false,
            _ => {}
        }
        if let Some(w) = transition_weight {
            weights[side] = w;
        }
        let events = match l.chop {
            Some(t) if l.tags.update(t) => l.tags.process(),
            Some(_) => Vec::new(),
            None if l.exiting => {
                l.exiting = false;
                l.tags.exit()
            }
            None => Vec::new(),
        };
        for (i, e) in events {
            match i {
                0 => CHOPPED_TAG.on(e, &mut chopped, &mut l.saved.0),
                1 => CHOP_RESET_TAG.on(e, &mut weights[1], &mut l.saved.1),
                _ => CHOP_RESET_TAG.on(e, &mut weights[0], &mut l.saved.2),
            }
        }
        (self.chop_weight, self.chopped) = (weights, chopped);
    }

    pub fn chop(&self) -> f32 {
        self.chop_weight[0].max(self.chop_weight[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> IceAxeTuning {
        IceAxeTuning {
            max_lean_angle: std::f32::consts::FRAC_PI_2,
            equilibrium_lean_angle: 1.396_263_4,
            lean_activation_threshold: 0.2,
            blend_activation_threshold: 0.9,
            chop_lean_threshold: 0.2,
            steering_filter_gain: 0.1,
            surface_weight_threshold: 0.6,
            noise_angle_normalization: std::f32::consts::FRAC_PI_6,
            steering_transform: Curve::new(vec![[0.0, 0.0], [1.0, 1.0]]),
            ..IceAxeTuning::default()
        }
    }

    #[test]
    fn spline_interpolates_uniform_samples() {
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 0.0), 0.0);
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 0.5), 0.5);
        assert!((tag_spline(&DEPLOY_SPLINE, 0.05) - 0.036_361_52).abs() < 1e-6);
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 1.0), 1.0);
    }

    #[test]
    fn the_axe_deploys_when_equipped() {
        let mut s = IceAxeState::default();
        let (mut blends, mut layer) = (Vec::new(), Vec::new());
        for _ in 0..32 {
            s.layers_update(true, 2.0);
            blends.push(s.blend_weight);
            layer.push(s.layer_weight);
        }
        assert!((layer[0] - tag_spline(&LAYER_SPLINE, 2.0 / 15.0)).abs() < 1e-6, "tick 2");
        assert!((layer[6] - tag_spline(&LAYER_SPLINE, 14.0 / 15.0)).abs() < 1e-6, "tick 14");
        assert_eq!(layer[7], layer[6], "tick 16: left");
        assert_eq!(layer[19], layer[6], "tick 40: the fade-out starts from it");
        assert_eq!(layer[24], 0.0, "tick 50");
        assert_eq!(blends[13], 0.0, "tick 28");
        assert_eq!(blends[14], 0.0, "tick 30: the window opens at weight 0");
        assert!((blends[15] - 0.172_361_37).abs() < 1e-6, "tick 32: s(0.2)");
        assert_eq!(blends[19], 1.0, "tick 40");
        assert!(s.deployed && s.deploy == DeployNode::Deployed);
        let mut bare = IceAxeState::default();
        bare.layers_update(false, 2.0);
        assert_eq!(bare.deploy, DeployNode::Holstered);
    }

    #[test]
    fn chops_and_bites() {
        let t = tuning();
        let mut s = IceAxeState { blend_weight: 1.0, ..IceAxeState::default() };
        let f = s.read_inputs(&t, 1.0, 0.9, 0.5, 0.3, 0.25, 2.0);
        assert!(f.blend == 1.0 && !f.active, "not chopped yet");
        s.chop_update(&t, 0.5);
        assert_eq!(s.start_chop, [false, true]);
        let mut chopped_at = None;
        let mut weights = Vec::new();
        for i in 0..16 {
            s.layers_update(false, 2.0);
            weights.push(s.chop_weight[1]);
            if s.chopped && chopped_at.is_none() {
                chopped_at = Some(i);
            }
        }
        assert_eq!(chopped_at, Some(2), "clip ticks 2, 4, 6");
        assert_eq!(weights[0], 1.0);
        assert_eq!(weights[12], 0.0, "clip tick 26");
        assert_eq!(weights[13], 0.0, "and stays down");
        assert!(s.layers[1].chopped_node && s.chop() == 0.0, "the hold is invisible");
        assert!(s.read_inputs(&t, 1.0, 0.9, 0.5, 0.3, 0.25, 2.0).active, "chopped and leaning: the bite");
        assert!(!s.read_inputs(&t, 1.0, 0.1, 0.5, 0.3, 0.25, 2.0).active, "off ice");
        s.chop_update(&t, 0.1);
        assert!(!s.chopped && s.start_chop == [false, false]);
        s.layers_update(false, 2.0);
        assert!(!s.layers[1].chopped_node && s.chop() == 0.0, "chopped → neutral keeps the arm down");
        let mut a = IceAxeState { blend_weight: 1.0, start_chop: [true, false], ..IceAxeState::default() };
        a.layers_update(false, 2.0);
        a.start_chop = [false, false];
        a.layers_update(false, 2.0);
        assert_eq!(a.chop_weight[0], 0.0, "chop → neutral");
    }
}
