use bevy::prelude::{
    App, FixedUpdate, IntoScheduleConfigs, PostUpdate, Quat, Resource, SystemSet, Update, Vec2, Vec3, World, info,
    resource_exists, warn,
};
use ssx_assets::postfx_overlay::OverlayDrop;
use ssx_physics::SIM_HZ;
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{Input, RiderState, TrickInput, World as PhysicsWorld};
use ssx_physics::session::{Execution, Session};

use crate::camera::CameraMode;
use crate::rider::skin::LoadedRider;
use crate::rider::visual::{self as rider_visual, LoadedBoard};
pub use clock::PausedFrames;
use clock::{fixed_step, frame_step, place_rider, report};
use controls::{PadLock, late_controls, read_controls};
use draw::{
    animate_rider, draw_rail_debug, hide_viewer_hud, light_gear, outro_fade, screen_effects, shade_rider,
    spawn_outro_fade, terrain_ripple, toggle_viewer_hud, world_render,
};
use hud::{GearHud, RiderLabel, spawn_hud, update_hud, update_trick_hud};
use scripted::AutoRewind;

pub mod bots;
mod clock;
mod controls;
mod draw;
pub mod helicopter;
mod hud;
pub mod live;
mod map_intro;
mod scripted;
mod start;
mod step;
mod strip;

pub use controls::read_live_controls;
pub use draw::TerrainRipple;

const TRIGGER_THRESHOLD: f32 = 0.5;
const CLASSIC_GRIND_HOLD_TICKS: f32 = 10.0;
const CLASSIC_REWIND_HOLD_TICKS: f32 = 12.0;
const CLASSIC_WINGSUIT_HOLD_TICKS: f32 = 8.0;

pub struct RideSetup {
    pub world: PhysicsWorld,
    pub spawn: (Vec3, Vec3),
    pub drop_start: bool,
    pub big_root: std::path::PathBuf,
    pub character: String,
    pub wire_key: u32,
    pub track: String,
    pub drop_key: Option<u32>,
    pub fixed_input: Option<Input>,
    pub jump_release: Vec<u32>,
    pub air_input: Option<Vec2>,
    pub grab: Option<Vec2>,
    pub tweak: bool,
    pub boost: bool,
    pub boost_meter: f32,
    pub screenshot_steps: Option<u32>,
    pub board: Option<LoadedBoard>,
    pub rider: Option<LoadedRider>,
    pub fly_camera: bool,
    pub load_secs: f32,
    pub event_key: Option<u32>,
    pub collected_geotags: Vec<u64>,
    pub game: bool,
    pub classic: bool,
    pub autoplay: bool,
    pub setup: Option<ssx_physics::gear::GameSetup>,
    pub intro_visits: Option<u32>,
    pub own_tombstone: Option<ssx_physics::session::tombstones::OwnRecord>,
    pub live: Option<live::LiveRules>,
    pub sim_content_id: Option<String>,
    pub map: Option<MapRide>,
}

#[derive(Clone, Debug)]
pub struct MapRide {
    pub id: ssx_content::ContentId,
    pub title: String,
    pub gates: ssx_physics::map::Gates,
    pub setup: ssx_physics::event::EventSetup,
    pub player: ssx_physics::map::MapPlayer,
    pub finished: bool,
    pub markers: Vec<MapMarker>,
    pub drawn: (u64, u64),
    pub mode: Option<MapRideMode>,
    pub heli: Option<std::sync::Arc<ssx_physics::map::MapHeli>>,
    pub placed: Vec<(u32, u32, ssx_assets::track_objects::Placement)>,
}

#[derive(Clone, Debug)]
pub struct MapRideMode {
    pub name: String,
    pub title: Option<String>,
    pub rules: ssx_content::gamemode::Rules,
    pub gamemode: Option<ssx_content::ContentId>,
}

impl MapRideMode {
    pub fn of(m: &ssx_assets::content::gamemodes::ResolvedMode) -> Self {
        MapRideMode {
            name: m.choice.name(),
            title: m.gamemode.as_ref().map(|g| g.value.title.clone()),
            rules: m.rules.clone(),
            gamemode: m.gamemode.as_ref().map(|g| g.id.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MapMarker {
    pub node: String,
    pub look: std::sync::Arc<ssx_assets::content::markers::MarkerLook>,
    pub position: [f32; 3],
    pub heading: f32,
}

impl MapRide {
    pub fn new(
        id: ssx_content::ContentId,
        title: String,
        gates: ssx_physics::map::Gates,
        setup: ssx_physics::event::EventSetup,
    ) -> MapRide {
        let fall_at =
            std::env::var("SSX_MAP_FALL_AT").ok().and_then(|v| v.parse::<f32>().ok()).filter(|t| t.is_finite());
        let player = ssx_physics::map::MapPlayer::new(fall_at);
        MapRide {
            id,
            title,
            gates,
            setup,
            player,
            finished: false,
            markers: Vec::new(),
            drawn: (0, 0),
            mode: None,
            heli: None,
            placed: Vec::new(),
        }
    }

    pub fn target(&self, world: &PhysicsWorld) -> Option<Vec3> {
        let gate = self.gates.checkpoints.get(self.gates.next).map(|v| v.centre);
        gate.or_else(|| self.gates.finish.and_then(|i| world.triggers.volumes.get(i)).map(|v| v.centre))
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RideStepSet;

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub traj: Vec3,
    pub traj_rot: Quat,
    pub board: Vec3,
    pub board_rot: Quat,
}

enum Clock {
    Fixed,
    PerFrame { started: bool, remaining: u32, sub: u32, drawn: u32, phase: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawnFrame {
    pub index: u32,
    pub phase: u32,
    pub sub: u32,
}

pub struct OutroRun {
    pub playing: bool,
    pub assets: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    pub reaction: Option<i64>,
}

#[derive(Resource)]
pub struct Ride {
    world: std::sync::Arc<PhysicsWorld>,
    spawn: (Vec3, Vec3),
    pub intro: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    intro_short: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    big_root: std::path::PathBuf,
    sim_content_id: Option<String>,
    pub map: Option<MapRide>,
    intro_graph: bool,
    pub cabin_seen: Option<usize>,
    pub world_hidden: bool,
    pub heli: Option<ssx_physics::helicopter::Helicopter>,
    heli_flight: Option<std::sync::Arc<ssx_physics::helicopter::HeliFlightData>>,
    pub vista: Option<ssx_physics::outro::VistaCamera>,
    event_def: Option<ssx_physics::event::EventDef>,
    character: String,
    wire_key: u32,
    pub outro: Option<OutroRun>,
    pub state: RiderState,
    prev: Pose,
    curr: Pose,
    step_ticks: f32,
    render_lag: f32,
    render_steps: u64,
    pub render: Pose,
    pub render_time: f32,
    pub steps: u64,
    pub distance: f64,
    pub input: Input,
    pub tricks: TrickInput,
    fixed_input: Option<Input>,
    jump_release: Vec<u32>,
    air_input: Option<Vec2>,
    grab: Option<Vec2>,
    tweak: bool,
    boost: bool,
    boost_meter: f32,
    r2: f32,
    clock: Clock,
    report: bool,
    pub step_ms: f32,
    pub session: Session,
    rewind_pad: RewindPad,
    pub rewinding: bool,
    l2: bool,
    grind_fixed: bool,
    pub rail_debug: bool,
    last_grind_capture: f32,
    pub rewind_backwards: bool,
    pub last_exec: Option<Execution>,
    press_script: Option<(bool, f32, f32)>,
    r1: bool,
    wingsuit_script: Option<(f32, f32)>,
    ws_pressed: Option<f32>,
    ws_deployed: Option<f32>,
    last_press_capture: f32,
    pub motion_blur_cue: i32,
    pub hold: bool,
    pub paused: bool,
    back_held: bool,
    back_was: bool,
    pub back_press: bool,
    autoplay: bool,
    classic: bool,
    classic_grind_ticks: f32,
    classic_rewind_ticks: f32,
    classic_wingsuit_ticks: f32,
    auto_rewind: AutoRewind,
    autoplay_waited: u64,
    course: Option<ssx_physics::harness::CoursePlayer>,
    course_logged: usize,
    course_start_at: f32,
    flare_pushes: usize,
    viewer_keys: bool,
    a_lock: PadLock,
    grind_since: Option<f32>,
    grind_ended: Option<f32>,
    messages_logged: f64,
    bail_diag_logged: Option<(f32, bool)>,
    pub loadout: ssx_assets::character::Loadout,
    pub game_setup: Option<ssx_physics::gear::GameSetup>,
    pub live: Option<live::LiveRules>,
    pub pads_held: bool,
    pub chat_held: bool,
    pub pose_blocks: Option<ssx_physics::graph::LiveBlocks>,
    pub start_gate: live::StartGate,
    pub go_step: Option<u64>,
}

impl Ride {
    pub fn sim_content_id(&self) -> Option<&str> {
        self.sim_content_id.as_deref()
    }

    pub fn big_root(&self) -> &std::path::Path {
        &self.big_root
    }

    pub fn world(&self) -> &PhysicsWorld {
        &self.world
    }

    pub fn set_classic(&mut self, classic: bool) {
        self.classic = classic;
        self.state.tricks.classic = classic;
        if !classic {
            (self.classic_grind_ticks, self.classic_rewind_ticks) = (0.0, 0.0);
            self.classic_wingsuit_ticks = 0.0;
        }
    }

    pub fn classic(&self) -> bool {
        self.classic
    }

    #[cfg(feature = "scripts")]
    pub fn world_arc(&self) -> std::sync::Arc<PhysicsWorld> {
        self.world.clone()
    }

    pub fn step(&self) -> (&Pose, f32) {
        (&self.curr, self.step_ticks)
    }

    pub fn drawn_frame(&self) -> Option<DrawnFrame> {
        match self.clock {
            Clock::PerFrame { started: true, sub, drawn, phase, .. } if drawn > 0 => {
                Some(DrawnFrame { index: drawn - 1, phase, sub })
            }
            _ => None,
        }
    }

    pub fn render_frac(&self) -> f32 {
        (self.render_time * SIM_HZ - (self.steps as f32 - 1.0)).clamp(0.0, 1.0)
    }

    pub fn live_clock(&self) -> bool {
        matches!(self.clock, Clock::Fixed)
    }

    pub fn seconds(&self) -> f32 {
        self.steps as f32 / SIM_HZ
    }

    pub fn character(&self) -> &str {
        &self.character
    }

    pub fn wire_key(&self) -> u32 {
        self.wire_key
    }
}

pub struct RideLoad {
    ride: Ride,
    board: Option<LoadedBoard>,
    body: Option<LoadedRider>,
    fly: bool,
    load_secs: f32,
    spawn: (Vec3, Vec3),
    overlay: Option<anyhow::Result<crate::render::post::overlay::OverlayLoad>>,
    avalanche: Option<crate::render::avalanche::AvalancheScene>,
    noise: Option<crate::camera::chase::CameraNoise>,
    ripple: Option<ssx_physics::controllers::ripple::RippleTuning>,
    label: String,
}

impl RideLoad {
    pub fn ride(&self) -> &Ride {
        &self.ride
    }

    pub fn ride_mut(&mut self) -> &mut Ride {
        &mut self.ride
    }

    pub fn avalanche(&self) -> Option<&crate::render::avalanche::AvalancheScene> {
        self.avalanche.as_ref()
    }

    pub fn heli_intro(&self) -> Option<std::sync::Arc<ssx_physics::nis::IntroAssets>> {
        self.ride.intro.clone().or_else(|| self.ride.heli.as_ref().map(|h| h.assets.clone()))
    }
}

pub fn prepare(
    mut setup: RideSetup,
    big_root: &std::path::Path,
    lighting: Option<&crate::render::lighting::GameLighting>,
) -> anyhow::Result<RideLoad> {
    use anyhow::Context;
    anyhow::ensure!(
        setup.live.is_none() || setup.screenshot_steps.is_none(),
        "a network session refuses the screenshot clock (one step per drawn frame): its steps follow the wall clock"
    );
    let board = setup.board.take();
    let mut body = setup.rider.take();
    let (load_secs, spawn, fly) = (setup.load_secs, setup.spawn, setup.fly_camera);
    let ride = Ride::new(setup).context("no ground within 100 m below the spawn point")?;
    let loadout = &ride.loadout;
    if let Some(b) = body.as_mut().filter(|_| *loadout != ssx_assets::character::Loadout::default()) {
        match b.dress(loadout) {
            Ok(()) => info!("Ride: dressed in {:?} {:?}", loadout.items, loadout.descriptions),
            Err(e) => warn!("Ride: the loadout's gear meshes failed ({e:#}); default outfit"),
        }
    }
    let s = &ride.world.survival;
    let drop = match (s.cold, s.low_oxygen, s.avalanche.avalanche_drop) {
        (true, _, _) => OverlayDrop::Cold,
        (false, true, _) => OverlayDrop::LowOxygen,
        (false, false, true) => OverlayDrop::Avalanche,
        _ => OverlayDrop::Default,
    };
    let presets = match ride.world.survival.objectives.as_deref() {
        Some(d) => {
            let event = ride.session.event.setup.objectives.as_ref().map(|e| &e.scripts);
            ssx_physics::controllers::postfx_presets(d, event)
        }
        None => Vec::new(),
    };
    let overlay = Some(crate::render::post::overlay::load(big_root, drop, &presets));
    let avalanche = lighting.and_then(|l| crate::render::avalanche::load(big_root, l.disc_track()?, &ride));
    let noise = crate::camera::chase::load_noise(big_root);
    let ripple = ssx_physics::controllers::ripple::RippleTuning::load(big_root)
        .map_err(|e| warn!("landing ripple unavailable: {e:#}"))
        .ok();
    let visual = match &board {
        Some(_) => "board: common_board_geo (player_board.fx drawn with the object_default pass)",
        None => "board: placeholder box (board mesh unavailable)",
    };
    let body_label = match &body {
        Some(_) => "body: skinned character, player_body/hair/eye/apparel listing port (rim + fill, dynamicAOMult)",
        None => "body: placeholder capsule (character unavailable)",
    };
    let label = format!("{visual} | {body_label}");
    Ok(RideLoad { ride, board, body, fly, load_secs, spawn, overlay, avalanche, noise, ripple, label })
}

pub fn insert(world: &mut World, r: RideLoad) -> anyhow::Result<()> {
    match r.overlay {
        Some(Ok(o)) => match crate::render::post::overlay::insert(world, o) {
            Ok(line) => info!("{line}"),
            Err(e) => warn!("screen effects unavailable: {e:#}"),
        },
        Some(Err(e)) => warn!("screen effects unavailable: {e:#}"),
        None => {}
    }
    if let Some(scene) = r.avalanche {
        world.insert_resource(scene);
    }
    if let Some(noise) = r.noise {
        world.insert_resource(noise);
    }
    world.insert_resource(GearHud::default());
    world.insert_resource(TerrainRipple { tuning: r.ripple, ..TerrainRipple::default() });
    if let Some(s) = strip::GrindStrip::from_env() {
        world.insert_resource(s);
    }
    let (load_secs, spawn) = (r.load_secs, r.spawn);
    info!("Ride: physics world built in {load_secs:.2}s; spawn {:.1} heading {:.2}", spawn.0, spawn.1);
    world.insert_resource(r.ride);
    world.insert_resource(RiderLabel(r.label));
    world.insert_resource(if r.fly { CameraMode::Fly } else { CameraMode::Chase });
    rider_visual::insert(world, r.board, r.body);
    Ok(())
}

pub fn register(app: &mut App, hud: bool) {
    let riding = resource_exists::<Ride>;
    app.init_resource::<PausedFrames>()
        .add_systems(FixedUpdate, fixed_step.in_set(RideStepSet).run_if(riding))
        .add_systems(
            Update,
            (
                read_controls,
                frame_step.in_set(RideStepSet),
                place_rider,
                crate::render::avalanche::update,
                shade_rider,
                animate_rider,
                light_gear,
                terrain_ripple,
                screen_effects,
                crate::camera::chase::toggle,
                crate::camera::chase::follow,
                report,
            )
                .chain()
                .run_if(riding),
        )
        .add_systems(PostUpdate, late_controls.run_if(riding))
        .add_systems(Update, draw_rail_debug.after(place_rider).run_if(riding))
        .add_systems(Update, strip::capture.after(report).run_if(riding))
        .add_systems(Update, world_render.after(place_rider))
        .add_systems(crate::shell::level::LevelStartup, spawn_outro_fade.run_if(riding))
        .add_systems(Update, outro_fade.after(place_rider).run_if(riding));
    crate::camera::chase::register(app);
    crate::render::avalanche::register(app);
    rider_visual::register(app);
    crate::shell::level::scoped::<Ride>(app);
    crate::shell::level::scoped::<GearHud>(app);
    crate::shell::level::scoped::<TerrainRipple>(app);
    crate::shell::level::scoped::<RiderLabel>(app);
    crate::shell::level::scoped::<CameraMode>(app);
    crate::shell::level::scoped::<strip::GrindStrip>(app);
    app.add_systems(crate::shell::level::LevelPostStartup, hide_viewer_hud.run_if(riding))
        .add_systems(Update, toggle_viewer_hud.run_if(riding));
    if hud {
        app.add_systems(crate::shell::level::LevelStartup, spawn_hud.run_if(riding)).add_systems(
            Update,
            (update_hud.after(animate_rider), update_trick_hud.after(animate_rider)).run_if(riding),
        );
    }
}
