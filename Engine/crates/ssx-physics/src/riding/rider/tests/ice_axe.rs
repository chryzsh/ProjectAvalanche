use super::*;
use crate::curve::Curve;
use crate::ice_axe::IceAxeTuning;

fn ice_slope(theta: f32) -> World {
    let half = 3000.0;
    let (sn, cs) = theta.sin_cos();
    let p = |x: f32, z: f32| Vec3::new(x, -z * sn, z * cs);
    let v = [p(-half, -half), p(-half, half), p(half, half), p(half, -half)];
    let mut c = CollisionWorld::default();
    c.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3]], 20, 0);
    c.finish();
    let mut w = world(c);
    w.survival.slots[crate::character::SLOT_ICE_AXE] = true;
    let snow = &w.tuning.carving;
    w.tuning.carving.ice_axe = IceAxeTuning {
        max_lean_angle: snow.max_lean_angle,
        equilibrium_lean_angle: snow.equilibrium_lean_angle,
        max_lean_input_curve: snow.max_lean_vs_speed_curve.clone(),
        lean_input_transform: snow.lean_input_angle_transform.clone(),
        lean_activation_threshold: 0.2,
        blend_activation_threshold: 0.9,
        chop_lean_threshold: 0.2,
        steering_filter_gain: 0.1,
        surface_weight_threshold: 0.6,
        noise_angle_normalization: std::f32::consts::FRAC_PI_6,
        steering_transform: Curve::new(vec![[0.0, 0.0], [1.0, 1.0]]),
        steering_transform_full: Curve::new(vec![[0.0, 0.0], [1.0, 1.0]]),
        steering_transform_blend: Curve::new(vec![[0.0, 0.0], [60.0, 1.0]]),
    };
    w
}

#[test]
fn the_axe_lets_go_when_the_turn_ends() {
    let theta = 15f32.to_radians();
    let w = ice_slope(theta);
    let at = Vec3::new(0.0, 2000.0 * theta.tan() + 1.0, -2000.0);
    let mut s = moving(&w, at, Vec3::Z, 0.25);
    ride(&w, &mut s, &Input::default(), 60, |_| {});
    assert!(s.track_started && s.ice_axe.deployed && s.ice_axe.blend_weight == 1.0, "{:?}", s.ice_axe);
    assert!(s.weights[ICE] > 0.9 && s.ice_axe.chop() == 0.0);
    let full = s.survival.ice_axe_effectiveness;
    let (mut swung, mut bit) = (false, false);
    ride(&w, &mut s, &Input { steer: 1.0, ..Input::default() }, 120, |s| {
        swung |= s.ice_axe.chop() == 1.0;
        bit |= s.ice_axe.frame.active;
        assert!(s.on_ground);
    });
    assert!(swung && bit, "{:?}", s.ice_axe);
    let after_swing = s.survival.ice_axe_effectiveness;
    assert!(after_swing < full, "the swing wears the axe: {after_swing} vs {full}");
    ride(&w, &mut s, &Input::default(), 120, |_| {});
    assert!(s.steering.lean_norm.abs() < 0.2, "leaning {}", s.steering.lean_norm);
    assert!(!s.ice_axe.chopped && !s.ice_axe.frame.active && s.ice_axe.start_chop == [false, false]);
    assert_eq!(s.ice_axe.chop_weight, [0.0, 0.0], "the arms are released: {:?}", s.ice_axe.layers);
    let released = s.survival.ice_axe_effectiveness;
    ride(&w, &mut s, &Input::default(), 60, |s| assert_eq!(s.ice_axe.chop(), 0.0));
    assert_eq!(s.survival.ice_axe_effectiveness, released, "no wear once released");
}
