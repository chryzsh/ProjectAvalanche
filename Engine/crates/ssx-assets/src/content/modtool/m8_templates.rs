use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_content::map::template::MAP_TOML;
use ssx_content::{BASE_NAMESPACE, ContentId, Kind};

const COURSE: &str = include_str!("../../../../../assets/templates/map/maps/course.gltf");

fn manifest(name: &str, description: &str, map: &str) -> String {
    format!(
        r#"# The manifest of the package "{name}" (Project Avalanche; every key: Docs/architecture/content.md §2, §22).
# Check it with `ssx-assets mod check <folder>`, pack it with `ssx-assets mod pack <folder>`.

[resource]
name = "{name}"
version = "0.1.0"
title = "{name}"
authors = []
description = "{description}"
api = 1
type = "mod"
priority = 0

# A map is its map.toml, which names its glTF scene; its id is "{name}:map/{map}".
[add]
"map/{map}" = "map.toml"
"#
    )
}

const MAP_README: &str = r#"# A map

`map.toml` names the scene `maps/course.gltf`; its nodes say what they are by their glTF `extras`
(Docs/architecture/content.md §22.3, Docs/architecture/modding.md §6.2):

- `slope` (`ava_type` "terrain", `ava_surface` "groomed"): the ground, 60 m wide, 360 m long, falling along +Z.
- `rail` ("rail", "metal"): a grindable rail beside the line.
- `spawn` ("spawn", `ava_slot` 0): where the rider starts; its local +Z is the heading.
- `checkpoint 1`, `checkpoint 2` ("checkpoint", `ava_order`, `ava_radius`): passed in order; the finish opens after the
  last one.
- `finish` ("finish"): the box the race ends in.
- `gate` ("marker", `ava_marker` "ava:marker/gate"): a gate drawn at the first checkpoint.
- `below` ("death"): a rider who falls off the slope dies here, and the level restarts at the spawn.
- `heli intro`, `heli pickup` ("heli_path", `ava_order` 0 and 1): the helicopter's two locators. With
  `intro = "heli"` in `map.toml` the ride starts in the helicopter as a disc drop's does: its intro flies to the
  spawn from the first, a survive-to-death run's pickup lands at the second after the finish (survive to death needs
  it). With `intro = "spawn"` (as made) they are not flown.
- `flare` ("prop", `ava_object` "ssx:object/markerflare_sml_red_gameobject"): a marker flare of the disc beside the
  line, an entity: its controller pushes it away from a rider passing within 2.5 m (Docs/architecture/content.md §29).

How it is drawn in the game (Docs/architecture/content.md §24): terrain and rails with a terrain material of the
disc (`ava_material` "ssx:material/<name>" on the node; `terrain_default_mat` by default, `terrain_rail_red_mat` on a
rail). A terrain node whose glTF material has a `baseColorTexture` (a PNG or JPEG of the package, placed by
`TEXCOORD_0`) is drawn with that texture as its base surface instead of the material's rock textures; the material's
snow, the snow tracks and the lighting stay. Group the terrain by texture: each texture is one material to draw. A
prop with its own mesh is drawn with its glTF material or `ava_material` naming a material of the package or the
disc; `midground` in `map.toml` shows a disc track's distant mountains around a new track. The game lists the map on
its Maps page.

Edit the scene in Blender (or any glTF 2.0 editor: keep the extras), then `ssx-assets mod check <folder>`. Ride it
headless: `cargo run --release -p ssx-physics --example ride_sim -- <big root> --map <folder>`.
"#;

pub(super) fn map(root: &Path, name: &str) -> Result<()> {
    let description = "A small course: a slope, a rail, two checkpoints and the finish";
    std::fs::write(root.join(ssx_content::manifest::MANIFEST_FILE), manifest(name, description, "course"))?;
    std::fs::write(root.join("map.toml"), MAP_TOML)?;
    std::fs::create_dir_all(root.join("maps"))?;
    std::fs::write(root.join("maps/course.gltf"), COURSE)?;
    std::fs::write(root.join("README.md"), MAP_README)?;
    Ok(())
}

fn gamemode_manifest(name: &str) -> String {
    format!(
        r#"# The manifest of the package "{name}" (Project Avalanche; every key: Docs/architecture/content.md §2, §26).
# Check it with `ssx-assets mod check <folder>`, pack it with `ssx-assets mod pack <folder>`.

[resource]
name = "{name}"
version = "0.1.0"
title = "{name}"
authors = []
description = "A race with medals on a small course"
api = 1
# A gamemode package: its gamemodes' rules are data the game and a server run without a script; its server scripts
# (a [scripts] server list) add to them, in single player in the game's local script host.
type = "gamemode"
priority = 0

# The map ("{name}:map/course") and the gamemode that races it ("{name}:gamemode/{gamemode}").
[add]
"map/course" = "map.toml"
"gamemode/{gamemode}" = "{path}"
{scripts}"#,
        gamemode = ssx_content::gamemode::template::NAME,
        path = ssx_content::gamemode::template::GAMEMODE_PATH,
        scripts = super::gamemode_script::MANIFEST_SCRIPTS,
    )
}

const GAMEMODE_README: &str = r#"# A gamemode

`gamemodes/gully_race.toml` races the map of this package (`map.toml`, its scene `maps/course.gltf`: the map
template's) against the clock for medals: its event type `race_ghosts` (the disc's Explore race), three targets in
seconds (gold, silver, bronze: the medal board a run is ranked on) and a time limit. Every key:
Docs/architecture/content.md §26.

The rules are data: the game and a server run them without a script. The Maps page lists the map with its built-in
modes (`map.toml` `modes`) and this gamemode; left / right choose the mode, A rides it. The results show the medal and
your best, kept in your profile (`<save>.mods.toml`).

Edit the targets after riding the course: `cargo run --release -p ssx-physics --example ride_sim -- <big root> --map
<folder> --mode <name>:gamemode/gully_race` prints the test player's time and the medal. Then `ssx-assets mod check
<folder>`.

`server/main.luau` adds to the rules (Docs/architecture/scripting.md §16): each gate's time against your best run on
the HUD, a solo run that falls `give_up_s` seconds (`[settings]`) behind the best at a gate ended, a faster finish kept
as the new best in the package's storage. In single player it runs in the game's local script host, on a server as one
of its resources. The server API's definitions are in `types/`; the map's next gate is drawn by the game.
"#;

pub(super) fn gamemode(root: &Path, name: &str) -> Result<()> {
    use ssx_content::gamemode::template::{GAMEMODE_PATH, gamemode_toml};
    std::fs::write(root.join(ssx_content::manifest::MANIFEST_FILE), gamemode_manifest(name))?;
    std::fs::write(root.join("map.toml"), MAP_TOML)?;
    std::fs::create_dir_all(root.join("maps"))?;
    std::fs::write(root.join("maps/course.gltf"), COURSE)?;
    let path = root.join(GAMEMODE_PATH);
    std::fs::create_dir_all(path.parent().context("the gamemode's folder")?)?;
    std::fs::write(&path, gamemode_toml(&format!("{name}:map/course")))?;
    std::fs::write(root.join("README.md"), GAMEMODE_README)?;
    super::gamemode_script::write(root)?;
    Ok(())
}

fn drop_id(drop: &str) -> Result<ContentId> {
    let text = if drop.contains(':') { drop.to_string() } else { format!("{BASE_NAMESPACE}:drop/{drop}") };
    let id: ContentId = text.parse().map_err(|e| anyhow::anyhow!("--drop {drop:?}: {e}"))?;
    if id.kind() != Kind::Drop || id.namespace() != BASE_NAMESPACE {
        bail!("--drop {drop:?}: a drop of the disc (`ssx-assets content ids --kind drop` lists them)");
    }
    Ok(id)
}

fn f(x: f32) -> String {
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
}

fn v<const N: usize>(a: [f32; N]) -> String {
    a.iter().map(|&x| f(x)).collect::<Vec<_>>().join(",")
}

pub(super) fn layout(name: &str, drop: &str, big: &Path) -> Result<Vec<(String, String)>> {
    let id = drop_id(drop)?;
    let base = crate::content::GameContent::of(big).layout_base(&id)?;
    let starts = ssx_rules::locators::load_drop_starts(big, &base.track)?;
    let slots = &starts.get(base.index).context("the drop's start locator")?.slots;
    let end = ssx_rules::locators::load_end_locator(big, &base.track, base.ge_drop)?;
    let mut nodes = Vec::new();
    for (k, s) in slots.iter().enumerate() {
        nodes.push(format!(
            "{{\"name\":\"start slot {k}\",\"translation\":[{}],\"rotation\":[{}],\"extras\":{{\"ava_type\":\"spawn\",\
             \"ava_slot\":{k}}}}}",
            v(s.position),
            v(s.rotation)
        ));
    }
    if let Some(e) = end {
        nodes.push(format!(
            "{{\"name\":\"the drop's end (stays unless a finish is added)\",\"translation\":[{}]}}",
            v(e.to_array())
        ));
    }
    let children = (1..=nodes.len()).map(|k| k.to_string()).collect::<Vec<_>>().join(",");
    let mut gltf = String::new();
    writeln!(
        gltf,
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"Project Avalanche layout template\"}},\"scene\":0,\
         \"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"name\":\"layout\",\"children\":[{children}]}},{}]}}",
        nodes.join(",")
    )?;
    let map_toml = format!(
        "# A layout on the disc drop {id} ({} drop {}; every key: Docs/architecture/content.md §22.3).\n[map]\n\
         title = \"Layout on {}\"\nscene = \"maps/layout.gltf\"\nbase = \"{id}\"\nmodes = [\"race\", \"freeride\"]\n",
        base.track, base.index, base.ui_drop
    );
    let readme = format!(
        "# A layout on {id}\n\nThe drop's own world stays: its terrain, its props, its boundary and its end. The scene \
         `maps/layout.gltf` holds the drop's start slots as this map's spawns (`ava_type` \"spawn\", read from the \
         drop's start locator on {} — move them, or delete all but slot 0) and an empty where the drop's end locator \
         stands. Add collision (`ava_type` \"collision\" with `ava_surface`), placed objects (\"prop\" with \
         `ava_object`), checkpoints, named triggers, death volumes, markers; a \"finish\" replaces the drop's end \
         (Docs/architecture/content.md §22.3). A layout adds no terrain and no boundary.\n\nRide it headless: \
         `cargo run --release -p ssx-physics --example ride_sim -- <big root> --map <folder>`.\n",
        base.track
    );
    Ok(vec![
        (ssx_content::manifest::MANIFEST_FILE.to_string(), manifest(name, &format!("A layout on {id}"), "layout")),
        ("map.toml".to_string(), map_toml),
        ("maps/layout.gltf".to_string(), gltf),
        ("README.md".to_string(), readme),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::{Template, new_layout, new_package};

    #[test]
    fn the_map_template_passes_the_package_check_and_a_layout_needs_a_disc_drop() {
        let out = std::env::temp_dir().join(format!("ssx-map-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let root = new_package("my-map", Template::Map, &out, std::path::Path::new("")).unwrap();
        let c = ssx_content::check::check_package(&root, &ssx_content::texture::Limits::default());
        let errors: Vec<String> = c.findings.iter().map(ToString::to_string).filter(|f| f.contains("error")).collect();
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(!c.findings.iter().any(|f| f.to_string().contains("phase M8b")), "{:#?}", c.findings);
        let (_, map) = &c.maps[&"my-map:map/course".parse().unwrap()];
        let d = &map.definition;
        assert_eq!((d.spawns.len(), d.checkpoints.len(), d.rails.len(), d.markers.len()), (1, 2, 1, 1));
        assert!(d.finish.is_some() && d.deaths.len() == 1);
        assert_eq!((d.heli_path.len(), d.props.len()), (2, 1));
        assert!(new_package("my-map2", Template::Layout, &out, std::path::Path::new("")).is_err());
        for drop in ["ssx:object/x", "pack:drop/x", "no such drop"] {
            assert!(new_layout("my-layout", drop, &out, &out.join("no-disc")).is_err(), "{drop}");
        }
        assert!(!out.join("my-layout").exists());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn the_gamemode_template_passes_the_package_check() {
        let out = std::env::temp_dir().join(format!("ssx-gamemode-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let root = new_package("my-mode", Template::Gamemode, &out, std::path::Path::new("")).unwrap();
        let c = ssx_content::check::check_package(&root, &ssx_content::texture::Limits::default());
        let errors: Vec<String> = c.findings.iter().map(ToString::to_string).filter(|f| f.contains("error")).collect();
        assert!(errors.is_empty(), "{errors:#?}");
        let text = std::fs::read_to_string(root.join(ssx_content::gamemode::template::GAMEMODE_PATH)).unwrap();
        assert_eq!(text, ssx_content::gamemode::template::gamemode_toml("my-mode:map/course"));
        let (_, g) = &c.gamemodes[&"my-mode:gamemode/gully_race".parse().unwrap()];
        let map = "my-mode:map/course".parse().unwrap();
        let rules = g.rules(&map, None, true).unwrap();
        assert_eq!(rules.board_values().len(), 3);
        assert!(c.maps.contains_key(&map));
        let m = c.manifest.as_ref().unwrap();
        assert_eq!(m.scripts.server.len(), 1);
        assert!(m.settings.contains_key("give_up_s"));
        assert!(root.join("server/main.luau").is_file() && root.join("types/ssx-api1-server.d.luau").is_file());
        let _ = std::fs::remove_dir_all(&out);
    }
}
