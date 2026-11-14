use bevy::prelude::*;
use ssx_physics::event::EventResult;
use ssx_ui::game::GameUi;
use ssx_ui::game::fe::Launch;

use super::LevelGame;
use crate::ride::Ride;

pub(super) fn post_results(
    ui: &mut GameUi,
    l: &Launch,
    r: &EventResult,
    course: Option<ssx_physics::event::Course>,
    o: RideOutcome,
) -> ssx_ui::game::ride::Posted {
    let RideOutcome { track_bonuses, bot_scores, splits, sim_content_id } = o;
    let ctx = ssx_ui::game::ride::ResultsContext {
        drop_key: l.drop,
        event_type: r.event_type.map_or(-1, |t| t as i32),
        event_name: l.event.clone(),
        character: l.rider.clone(),
        track_bonuses,
        bot_scores,
        event_id: ssx_assets::content::data::events::event_id(&l.event),
        sim_content_id,
    };
    let Some((mut profile, rules)) = ui.fe.as_mut().map(|f| (std::mem::take(&mut f.profile), f.rules.clone())) else {
        return ssx_ui::game::ride::Posted { won: false, best_not_kept: None, medal: None, new_best: false };
    };
    use ssx_assets::career::profile::DropState;
    let before = profile.drop_state(l.drop);
    let posted = ssx_ui::game::ride::post_event_results_with_splits(ui, &ctx, r, splits, &mut profile, &rules);
    if r.failed {
        ssx_ui::game::ride::post_death_body(ui, course, r);
    }
    if before == DropState::Unlocked && profile.drop_state(l.drop) == DropState::Completed {
        ssx_ui::game::fe::flow::campaign_drop_completed(&mut ui.values, l.drop);
    }
    if let Some(f) = ui.fe.as_mut() {
        f.profile = profile;
        ssx_ui::game::fe::flow::store_camera_type(f);
        f.last_success = Some(posted.won);
        f.last_result_drop = l.drop;
    }
    posted
}

pub(super) struct RideOutcome {
    pub track_bonuses: Vec<String>,
    pub bot_scores: Vec<u32>,
    pub splits: [u32; 4],
    pub sim_content_id: Option<String>,
}

pub(super) fn sync_named_flags(l: &mut LevelGame, ride: &mut Ride, ui: &mut GameUi) {
    let Some(fe) = ui.fe.as_mut() else { return };
    let scripts: std::collections::BTreeSet<u32> = ride.session.permanent_named_flags().into_iter().collect();
    if scripts == l.flags && fe.profile.named_flags == l.flags {
        return;
    }
    let changes = |now: &std::collections::BTreeSet<u32>| -> Vec<(u32, bool)> {
        let set = now.difference(&l.flags).map(|&k| (k, true));
        set.chain(l.flags.difference(now).map(|&k| (k, false))).collect()
    };
    let (by_scripts, by_front_end) = (changes(&scripts), changes(&fe.profile.named_flags));
    for &(k, on) in &by_scripts {
        if on {
            fe.profile.named_flags.insert(k);
        } else {
            fe.profile.named_flags.remove(&k);
        }
    }
    for &(k, on) in &by_front_end {
        ride.session.set_named_flag(k, on);
    }
    if !by_scripts.is_empty() {
        let list: Vec<String> =
            by_scripts.iter().map(|(k, on)| format!("{k:#010x} {}", if *on { "set" } else { "cleared" })).collect();
        info!("level: permanent named flags by the scripts at ride time {:.2} s: {}", ride.seconds(), list.join(", "));
    }
    l.flags = fe.profile.named_flags.clone();
}

pub fn stats_from_ride(ui: &mut GameUi, ride: &Ride, names: &std::collections::BTreeMap<u32, String>) {
    let Some(fe) = ui.fe.as_mut() else { return };
    let new: Vec<ssx_assets::career::profile::BadgeText> =
        ssx_ui::game::stats::from_session(&mut fe.profile, &ride.session.stats, names, &fe.rules)
            .into_iter()
            .cloned()
            .collect();
    for b in &new {
        info!("level: badge {} awarded ({})", b.name, b.title);
        ssx_ui::game::fe::notify::post_badge(fe, &mut ui.loc, b);
    }
    fe.badge_progress.extend(ride.session.stats.badge_progress.iter().map(|(&k, &v)| (k, v)));
}
