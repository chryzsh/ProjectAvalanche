use std::collections::BTreeMap;

use super::*;
use crate::model::FileReader;

struct Files(BTreeMap<String, Vec<u8>>);

impl FileReader for Files {
    fn read(&self, path: &str, max: u64) -> Result<Vec<u8>, String> {
        let b = self.0.get(path).ok_or_else(|| format!("{path}: not in the package"))?;
        if b.len() as u64 > max { Err(format!("{path}: over {max} bytes")) } else { Ok(b.clone()) }
    }

    fn size(&self, path: &str) -> Result<u64, String> {
        Ok(self.0.get(path).ok_or_else(|| format!("{path}: not in the package"))?.len() as u64)
    }

    fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let b = self.0.get(path).ok_or_else(|| format!("{path}: not in the package"))?;
        let (o, l) =
            (usize::try_from(offset).map_err(|e| e.to_string())?, usize::try_from(len).map_err(|e| e.to_string())?);
        b.get(o..o.saturating_add(l)).map(<[u8]>::to_vec).ok_or_else(|| format!("{path}: short"))
    }
}

#[derive(Default)]
struct G {
    bin: Vec<u8>,
    accessors: Vec<String>,
    views: Vec<String>,
    meshes: Vec<String>,
    nodes: Vec<String>,
    materials: Vec<String>,
    images: Vec<String>,
}

impl G {
    fn view(&mut self, bytes: &[u8]) -> usize {
        let start = self.bin.len();
        self.bin.extend(bytes);
        self.views.push(format!("{{\"buffer\":0,\"byteOffset\":{start},\"byteLength\":{}}}", bytes.len()));
        self.views.len() - 1
    }

    fn mesh(&mut self, positions: &[[f32; 3]], indices: &[u32]) -> usize {
        let p: Vec<u8> = positions.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        let v = self.view(&p);
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for q in positions {
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let num = |x: f32| if x.is_finite() { format!("{x:?}") } else { "0.0".into() };
        self.accessors.push(format!(
            "{{\"bufferView\":{v},\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}}",
            positions.len(),
            num(lo[0]),
            num(lo[1]),
            num(lo[2]),
            num(hi[0]),
            num(hi[1]),
            num(hi[2])
        ));
        let pa = self.accessors.len() - 1;
        let i: Vec<u8> = indices.iter().flat_map(|x| x.to_le_bytes()).collect();
        let iv = self.view(&i);
        self.accessors.push(format!(
            "{{\"bufferView\":{iv},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}",
            indices.len()
        ));
        let ia = self.accessors.len() - 1;
        self.meshes.push(format!("{{\"primitives\":[{{\"attributes\":{{\"POSITION\":{pa}}},\"indices\":{ia}}}]}}"));
        self.meshes.len() - 1
    }

    fn textured_quad(&mut self, uri: &str, uvs: bool) -> usize {
        let m = self.quad(50.0, 0.0);
        self.images.push(format!("{{\"uri\":\"{uri}\"}}"));
        let image = self.images.len() - 1;
        self.materials.push(format!("{{\"pbrMetallicRoughness\":{{\"baseColorTexture\":{{\"index\":{image}}}}}}}"));
        let material = self.materials.len() - 1;
        let uv = if uvs {
            let data: Vec<u8> = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
                .iter()
                .flatten()
                .flat_map(|x| x.to_le_bytes())
                .collect();
            let v = self.view(&data);
            self.accessors.push(format!("{{\"bufferView\":{v},\"componentType\":5126,\"count\":4,\"type\":\"VEC2\"}}"));
            format!(",\"TEXCOORD_0\":{}", self.accessors.len() - 1)
        } else {
            String::new()
        };
        self.meshes[m] =
            self.meshes[m].replacen("},\"indices\"", &format!("{uv}}},\"material\":{material},\"indices\""), 1);
        m
    }

    fn quad(&mut self, half: f32, y: f32) -> usize {
        let p = [[-half, y, -half], [half, y, -half], [half, y, half], [-half, y, half]];
        self.mesh(&p, &[0, 3, 1, 1, 3, 2])
    }

    fn node(&mut self, json: &str) -> usize {
        self.nodes.push(json.to_string());
        self.nodes.len() - 1
    }

    fn text(&self, roots: &[usize]) -> String {
        let roots: Vec<String> = roots.iter().map(ToString::to_string).collect();
        let textures: Vec<String> = (0..self.images.len()).map(|i| format!("{{\"source\":{i}}}")).collect();
        format!(
            "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\"nodes\":[{}],\"meshes\":[{}],\
             \"materials\":[{}],\"textures\":[{}],\"images\":[{}],\
             \"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{},\"uri\":\"data:application/octet-stream;base64,{}\"}}]}}",
            roots.join(","),
            self.nodes.join(","),
            self.meshes.join(","),
            self.materials.join(","),
            textures.join(","),
            self.images.join(","),
            self.accessors.join(","),
            self.views.join(","),
            self.bin.len(),
            template::base64(&self.bin)
        )
    }
}

const RACE: &str = "[map]\ntitle = \"T\"\nscene = \"s.gltf\"\nmodes = [\"race\"]\n";

fn read(toml: &str, scene: &str) -> Result<Map, MapError> {
    read_with(toml, scene, &[])
}

fn read_with(toml: &str, scene: &str, more: &[(&str, Vec<u8>)]) -> Result<Map, MapError> {
    let mut files = Files([("s.gltf".to_string(), scene.as_bytes().to_vec())].into_iter().collect());
    files.0.extend(more.iter().map(|(name, bytes)| (name.to_string(), bytes.clone())));
    read_map(toml.as_bytes(), "map.toml", &files)
}

fn refused(toml: &str, scene: &str, want: &str) -> MapError {
    match read(toml, scene) {
        Ok(_) => panic!("accepted; wanted {want:?}"),
        Err(e) => {
            assert!(e.to_string().contains(want), "{e} does not say {want:?}");
            e
        }
    }
}

fn race(more: impl FnOnce(&mut G) -> Vec<usize>) -> String {
    let mut g = G::default();
    let ground = g.quad(50.0, 0.0);
    let cube = g.mesh(
        &[
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
        ],
        &[0, 2, 1, 0, 3, 2, 4, 5, 2],
    );
    let mut kids = vec![
        g.node(&format!("{{\"name\":\"ground\",\"mesh\":{ground},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"powder\"}}}}")),
        g.node("{\"name\":\"start\",\"translation\":[0,1,-40],\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}"),
        g.node(&format!(
            "{{\"name\":\"finish\",\"mesh\":{cube},\"translation\":[0,2,40],\"scale\":[20,4,2],\"extras\":{{\"ava_type\":\"finish\"}}}}"
        )),
    ];
    kids.extend(more(&mut g));
    let list: Vec<String> = kids.iter().map(ToString::to_string).collect();
    let root = g.node(&format!("{{\"name\":\"course\",\"children\":[{}]}}", list.join(",")));
    g.text(&[root])
}

#[test]
fn a_small_race_reads_in_the_world_frame() {
    let scene = race(|g| {
        vec![
            g.node("{\"name\":\"cp\",\"translation\":[0,1,0],\"extras\":{\"ava_type\":\"checkpoint\",\"ava_order\":0,\"ava_radius\":5}}"),
            g.node("{\"name\":\"lake\",\"translation\":[0,-10,0],\"extras\":{\"ava_type\":\"death\",\"ava_radius\":30}}"),
        ]
    });
    let m = read(RACE, &scene).expect("reads");
    let d = &m.definition;
    assert_eq!((d.terrain.len(), d.spawns.len(), d.checkpoints.len(), d.deaths.len()), (1, 1, 1, 1));
    assert_eq!(d.terrain[0].weights[0], TERRAIN_SURFACES[0].1);
    assert_eq!(d.spawns[0].position, [0.0, 1.0, -40.0]);
    assert_eq!(d.spawns[0].node, "course/start");
    let f = d.finish.as_ref().expect("a finish");
    assert_eq!(f.name, "end");
    match f.shape {
        VolumeShape::Box { centre, half_extents, .. } => {
            assert_eq!(centre, [0.0, 2.0, 40.0]);
            assert_eq!(half_extents, [10.0, 2.0, 1.0]);
        }
        VolumeShape::Sphere { .. } => panic!("a box"),
    }
    assert_eq!(sim_hash(&m), sim_hash(&read(RACE, &scene).expect("reads")), "one hash for one map");
    let lit = read(&format!("{RACE}lighting = \"ssx:lighting/night_clear_lighting\"\n"), &scene).expect("reads");
    let sky = read(&format!("{RACE}sky = \"pack:sky/aurora\"\n"), &scene).expect("reads");
    assert_ne!(sim_hash(&lit), sim_hash(&m));
    assert_eq!(sim_hash(&sky), sim_hash(&m));
}

fn textured_race(uvs: bool) -> String {
    let mut g = G::default();
    let ground = g.textured_quad("tex.png", uvs);
    let kids = [
        g.node(&format!(
            "{{\"name\":\"ground\",\"mesh\":{ground},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"rock\"}}}}"
        )),
        g.node("{\"name\":\"start\",\"translation\":[0,1,-40],\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}"),
        g.node("{\"name\":\"end\",\"translation\":[0,1,40],\"extras\":{\"ava_type\":\"finish\",\"ava_radius\":5}}"),
    ];
    let list: Vec<String> = kids.iter().map(ToString::to_string).collect();
    let root = g.node(&format!("{{\"name\":\"course\",\"children\":[{}]}}", list.join(",")));
    g.text(&[root])
}

#[test]
fn a_terrain_with_a_base_colour_texture_keeps_its_uvs_and_image() {
    let png = crate::test_dir::png_rgba(4, 4, [120, 90, 60, 255]);
    let m = read_with(RACE, &textured_race(true), &[("tex.png", png)]).expect("reads");
    let t = &m.definition.terrain[0];
    let texture = t.texture.as_ref().expect("a texture");
    assert_eq!((texture.material, texture.uv.len()), (0, 4));
    assert_eq!(texture.uv[2], [1.0, 1.0]);
    assert_eq!(m.terrain_image(t), Some(0));
    let props = m.props.as_ref().expect("the scene's images are decoded");
    assert_eq!((props.model.images.len(), props.ranges.len(), props.cost.vertices), (1, 0, 0));
    assert!(props.cost.drawn_image_bytes > 0);
    let mut g = G::default();
    let plain = g.quad(50.0, 0.0);
    let kids = [
        g.node(&format!(
            "{{\"name\":\"ground\",\"mesh\":{plain},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"rock\"}}}}"
        )),
        g.node("{\"name\":\"start\",\"translation\":[0,1,-40],\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}"),
        g.node("{\"name\":\"end\",\"translation\":[0,1,40],\"extras\":{\"ava_type\":\"finish\",\"ava_radius\":5}}"),
    ];
    let list: Vec<String> = kids.iter().map(ToString::to_string).collect();
    let root = g.node(&format!("{{\"name\":\"course\",\"children\":[{}]}}", list.join(",")));
    let untextured = read(RACE, &g.text(&[root])).expect("reads");
    assert!(untextured.props.is_none() && untextured.definition.terrain[0].texture.is_none());
    assert_eq!(sim_hash(&m), sim_hash(&untextured), "a texture is drawn, not simulated");
}

#[test]
fn a_terrain_texture_that_is_missing_or_unplaced_is_refused() {
    let e = read_with(RACE, &textured_race(true), &[]).expect_err("the texture file is missing");
    assert!(e.to_string().contains("tex.png"), "{e}");
    let png = crate::test_dir::png_rgba(4, 4, [120, 90, 60, 255]);
    let e = read_with(RACE, &textured_race(false), &[("tex.png", png)]).expect_err("no TEXCOORD_0");
    assert!(e.to_string().contains("TEXCOORD_0") && e.to_string().contains("course/ground"), "{e}");
}

#[test]
fn the_package_check_accepts_a_textured_terrain_and_refuses_a_missing_texture() {
    use crate::check::{Severity, check_package};
    use crate::test_dir::TestDir;
    let manifest = "[resource]\nname = \"pack\"\nversion = \"1.0.0\"\napi = 1\n[add]\n\"map/t\" = \"map.toml\"\n";
    let d = TestDir::new("map-texture");
    d.write("resource.toml", manifest);
    d.write("map.toml", RACE);
    d.write("s.gltf", &textured_race(true));
    d.write_bytes("tex.png", &crate::test_dir::png_rgba(4, 4, [120, 90, 60, 255]));
    let c = check_package(&d.0, &crate::texture::Limits::default());
    assert_eq!(c.errors(), 0, "{:#?}", c.findings);
    let (_, map) = &c.maps[&"pack:map/t".parse().unwrap()];
    assert_eq!(map.terrain_image(&map.definition.terrain[0]), Some(0));
    std::fs::remove_file(d.0.join("tex.png")).expect("removed");
    let c = check_package(&d.0, &crate::texture::Limits::default());
    let errors: Vec<String> = c
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| format!("{}: {}", f.file, f.message))
        .collect();
    assert!(errors.iter().any(|e| e.contains("tex.png")), "{errors:#?}");
    assert!(!c.maps.contains_key(&"pack:map/t".parse().unwrap()));
}

#[test]
fn the_template_reads_and_its_committed_copy_is_the_generated_one() {
    let toml = template::MAP_TOML;
    let files = Files([("maps/course.gltf".to_string(), template::gltf().into_bytes())].into_iter().collect());
    let m = read_map(toml.as_bytes(), "map.toml", &files).expect("the template reads");
    let d = &m.definition;
    assert_eq!((d.spawns.len(), d.checkpoints.len(), d.rails.len(), d.markers.len()), (1, 2, 1, 1));
    assert!(d.finish.is_some() && d.deaths.len() == 1 && d.terrain.len() == 1);
    assert_eq!(d.rails[0].triangles[0].group, RAIL_GROUP);
    assert_eq!(d.heli_path.iter().map(|p| p.order).collect::<Vec<_>>(), vec![0, 1]);
    assert!(d.notes.iter().any(|n| n.contains("flown with intro = \"heli\"")), "{:?}", d.notes);
    assert!(matches!(&d.props[..], [p] if p.look == PropLook::Object(template::FLARE.parse().unwrap())));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/templates/map");
    let committed = std::fs::read_to_string(dir.join("maps/course.gltf")).unwrap_or_default();
    assert!(
        committed == template::gltf(),
        "maps/course.gltf differs from the generator: run `cargo run -p ssx-content --example map_template`"
    );
}

#[test]
fn a_mirrored_node_keeps_its_triangles_facing_up() {
    let scene = race(|g| {
        let q = g.quad(5.0, 0.0);
        vec![g.node(&format!(
            "{{\"name\":\"ledge\",\"mesh\":{q},\"scale\":[-1,1,1],\"extras\":{{\"ava_type\":\"collision\",\"ava_surface\":\"wood\"}}}}"
        ))]
    });
    let m = read(RACE, &scene).expect("reads");
    let t = &m.definition.collision[0].triangles[0].corners;
    let (u, v) = ([t[1][0] - t[0][0], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][2] - t[0][2]]);
    assert!(u[1] * v[0] - u[0] * v[1] > 0.0, "counter-clockwise from above");
}

#[test]
fn hostile_scenes_are_refused_with_the_node_and_key() {
    let nan = race(|g| {
        let m = g.mesh(&[[0.0, f32::NAN, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], &[0, 2, 1]);
        vec![g.node(&format!(
            "{{\"name\":\"bad\",\"mesh\":{m},\"extras\":{{\"ava_type\":\"collision\",\"ava_surface\":\"wood\"}}}}"
        ))]
    });
    refused(
        RACE,
        &nan,
        "node \"course/bad\": meshes[2].primitives[0]: POSITION: accessors[4]: a value that is not a finite number",
    );
    let far =
        race(|g| {
            vec![g.node(
                "{\"name\":\"far\",\"translation\":[200000,0,0],\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":1}}",
            )]
        });
    refused(RACE, &far, "node \"course/far\": a position beyond");
    let unknown =
        race(|g| vec![g.node("{\"name\":\"x\",\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":1,\"ava_colour\":1}}")]);
    refused(RACE, &unknown, "node \"course/x\" ava_colour: not one of the map's keys");
    let wrong =
        race(|g| vec![g.node("{\"name\":\"x\",\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":1,\"ava_radius\":1}}")]);
    refused(RACE, &wrong, "ava_radius: not read on a spawn node");
    let twice = race(|g| vec![g.node("{\"name\":\"x\",\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}")]);
    refused(RACE, &twice, "slot 0 twice");
    let untyped = race(|g| {
        let q = g.quad(1.0, 0.0);
        vec![g.node(&format!("{{\"name\":\"y\",\"mesh\":{q}}}"))]
    });
    refused(RACE, &untyped, "node \"course/y\" ava_type: a mesh without ava_type");
    let big = format!("{{\"name\":\"x\",\"extras\":{{\"note\":\"{}\"}}}}", "a".repeat(1 << 20));
    refused(RACE, &race(|g| vec![g.node(&big)]), "at most 4096 are read");
    let end = race(|g| {
        vec![g.node("{\"name\":\"t\",\"extras\":{\"ava_type\":\"trigger\",\"ava_name\":\"end\",\"ava_radius\":2}}")]
    });
    refused(RACE, &end, "\"end\" is the finish's name");
}

#[test]
fn hostile_node_names_are_escaped_and_paths_bounded() {
    let esc =
        race(|g| vec![g.node("{\"name\":\"a\\u001b[2J\\\"b\",\"extras\":{\"ava_type\":\"spawn\",\"ava_colour\":1}}")]);
    let e = refused(RACE, &esc, "node \"course/a\\u{1b}[2J\\\"b\" ava_colour").to_string();
    assert!(!e.contains('\u{1b}'), "{e:?}");
    let deep = race(|g| {
        let mut child = g.node("{\"name\":\"leaf\",\"extras\":{\"ava_type\":\"spawn\",\"ava_colour\":1}}");
        for k in 0..60 {
            child = g.node(&format!("{{\"name\":\"{}{k}\",\"children\":[{child}]}}", "x".repeat(100)));
        }
        vec![child]
    });
    let e = refused(RACE, &deep, "/leaf\" ava_colour").to_string();
    assert!(e.contains("node \"…/") && e.chars().count() < 1024, "{} characters: {e}", e.chars().count());
    let cameras = race(|g| (0..100).map(|k| g.node(&format!("{{\"name\":\"c{k}\",\"camera\":0}}"))).collect())
        .replacen(
            "\"scene\":0,",
            "\"cameras\":[{\"type\":\"perspective\",\"perspective\":{\"yfov\":1.0,\"znear\":0.1}}],\"scene\":0,",
            1,
        );
    let map = read(RACE, &cameras).expect("cameras without ava_type are not refused");
    assert_eq!(
        map.definition.notes,
        vec!["100 cameras without ava_type are not used (the first: node \"course/c0\")".to_string()]
    );
}

#[test]
fn cycles_shared_nodes_and_deep_trees_are_refused() {
    let mut g = G::default();
    let a = g.node("{\"name\":\"a\",\"children\":[1]}");
    g.node("{\"name\":\"b\",\"children\":[0]}");
    refused(RACE, &g.text(&[a]), "reached twice");
    let mut g = G::default();
    g.node("{\"name\":\"shared\"}");
    g.node("{\"children\":[0]}");
    g.node("{\"children\":[0]}");
    refused(RACE, &g.text(&[1, 2]), "nodes[0] is reached twice");
    let mut g = G::default();
    for i in 0..10_000 {
        let next = if i + 1 < 10_000 { format!(",\"children\":[{}]", i + 1) } else { String::new() };
        g.node(&format!("{{\"name\":\"n{i}\"{next}}}"));
    }
    refused(RACE, &g.text(&[0]), "10000 nodes; at most 4096");
    let mut g = G::default();
    for i in 0..100 {
        let next = if i + 1 < 100 { format!(",\"children\":[{}]", i + 1) } else { String::new() };
        g.node(&format!("{{\"name\":\"n{i}\"{next}}}"));
    }
    refused(RACE, &g.text(&[0]), "nested deeper than 64");
}

#[test]
fn a_map_without_its_requirements_is_refused() {
    let mut g = G::default();
    let q = g.quad(10.0, 0.0);
    let ground = g.node(&format!("{{\"mesh\":{q},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"ice\"}}}}"));
    refused(RACE, &g.text(&[ground]), "a new track has a spawn");
    let spawn = g.node("{\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}");
    refused(RACE, &g.text(&[ground, spawn]), "a race map needs a finish");
    let trick = RACE.replace("[\"race\"]", "[\"trick\"]");
    refused(&trick, &g.text(&[ground, spawn]), "a trick map needs a finish");
    let freeride = RACE.replace("[\"race\"]", "[\"freeride\"]");
    assert!(read(&freeride, &g.text(&[ground, spawn])).is_ok(), "a freeride map needs no finish");
    let heli = format!("{freeride}intro = \"heli\"\n");
    refused(&heli, &g.text(&[ground, spawn]), "on a new track needs a heli_path");
    let layout = format!("{freeride}base = \"ssx:drop/alps0101\"\n");
    refused(&layout, &g.text(&[ground, spawn]), "adds no terrain");
}

#[test]
fn the_helicopters_path_is_its_two_locators() {
    let mut g = G::default();
    let q = g.quad(10.0, 0.0);
    let ground = g.node(&format!("{{\"mesh\":{q},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"ice\"}}}}"));
    let spawn = g.node("{\"extras\":{\"ava_type\":\"spawn\",\"ava_slot\":0}}");
    let point = |g: &mut G, order: u32, x: f32| {
        g.node(&format!(
            "{{\"translation\":[{x},5,0],\"rotation\":[0,0.7071068,0,0.7071068],\
             \"extras\":{{\"ava_type\":\"heli_path\",\"ava_order\":{order}}}}}"
        ))
    };
    let (p0, p1, p2) = (point(&mut g, 0, -3.0), point(&mut g, 1, 4.0), point(&mut g, 2, 9.0));
    let p1b = point(&mut g, 1, 6.0);
    let heli = RACE.replace("[\"race\"]", "[\"freeride\"]") + "intro = \"heli\"\n";
    let m = read(&heli, &g.text(&[ground, spawn, p0, p1])).expect("two locators read");
    let path: Vec<(u32, [f32; 3])> = m.definition.heli_path.iter().map(|p| (p.order, p.position)).collect();
    assert_eq!(path, vec![(0, [-3.0, 5.0, 0.0]), (1, [4.0, 5.0, 0.0])]);
    assert!(
        (m.definition.heli_path[1].rotation[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
        "the locator keeps its rotation"
    );
    refused(&heli, &g.text(&[ground, spawn, p0, p1, p2]), "0 (the intro locator) or 1 (the outro locator)");
    refused(&heli, &g.text(&[ground, spawn, p1]), "order 0 (nis_intro_locator) is missing");
    refused(&heli, &g.text(&[ground, spawn, p0]), "order 1 (nis_outro_locator) is missing");
    refused(&heli, &g.text(&[ground, spawn, p0, p1, p1b]), "order 1 twice");
    refused(&heli, &g.text(&[ground, spawn]), "on a new track needs a heli_path");
    let far = g.node("{\"translation\":[0,5,140000],\"extras\":{\"ava_type\":\"heli_path\",\"ava_order\":0}}");
    refused(&heli, &g.text(&[ground, spawn, far, p1]), "a position beyond");
    let text = g.text(&[ground, spawn, p0, p1]);
    assert_eq!(text.matches("[-3,5,0]").count(), 1);
    let inf = text.replace("[-3,5,0]", "[0,1e999,0]");
    assert!(read(&heli, &inf).is_err(), "an overflowing number is refused");
    let death = RACE.replace("[\"race\"]", "[\"freeride\", \"death\"]");
    let e = parse_toml(death.as_bytes()).expect_err("a death map at its spawn");
    assert!(e[0].0 == 4 && e[0].2.contains("a death map starts in the helicopter"), "{e:?}");
    let spawn_map = RACE.replace("[\"race\"]", "[\"freeride\"]");
    let m = read(&spawn_map, &g.text(&[ground, spawn, p0, p1])).expect("a spawn intro with a path reads");
    assert!(m.definition.notes.iter().any(|n| n.contains("flown with intro = \"heli\"")), "{:?}", m.definition.notes);
    let moved = read(&heli, &g.text(&[ground, spawn, p0, p1b])).expect("reads");
    let first = read(&heli, &g.text(&[ground, spawn, p0, p1])).expect("reads");
    assert_ne!(sim_hash(&moved), sim_hash(&first));
}

#[test]
fn map_toml_problems_have_their_lines() {
    let bad = "[map]\ntitle = \"\"\nscene = \"../x.glb\"\nbase = \"pack:drop/a\"\nmodes = [\"race\", \"race\", \"golf\"]\n\
               time_limit_s = 4000\nintro = \"walk\"\nlighting = \"ssx:sky/x\"\n";
    let problems = parse_toml(bad.as_bytes()).expect_err("refused");
    let lines: Vec<usize> = problems.iter().map(|p| p.0).collect();
    assert_eq!(lines, vec![2, 3, 4, 5, 5, 6, 7, 8], "{problems:?}");
    assert!(parse_toml(b"[map]\ntitle = \"x\"\nscene = \"a.glb\"\nmodes = [\"trick\"]\ncolour = 1\n").is_err());
    let own = "[map]\ntitle = \"x\"\nscene = \"a.glb\"\nmodes = [\"race\"]\nlighting = \"pack:lighting/x\"\n";
    let p = parse_toml(own.as_bytes()).expect_err("refused");
    assert!(p[0].0 == 5 && p[0].2.contains("a preset of the disc"), "{p:?}");
    let ok = parse_toml(template::MAP_TOML.as_bytes()).expect("the template's record reads");
    assert_eq!(ok.modes, vec![Mode::Race, Mode::Freeride]);
}

#[test]
fn presentation_references_follow_the_games_kinds() {
    let with = |more: &str| format!("{RACE}{more}\n");
    let problem = |more: &str| {
        let p = parse_toml(with(more).as_bytes()).expect_err("refused");
        p.into_iter().map(|p| p.2).collect::<Vec<_>>().join("; ")
    };
    assert!(problem("postfx = \"pack:postfx/x\"").contains("a preset of the disc"));
    assert!(problem("music = \"ssx:music/pathfinder/ssxdd/0\"").contains("a song of the disc"));
    assert!(problem("midground = \"pack:track/x\"").contains("a track of the disc"));
    assert!(problem("base = \"ssx:drop/a\"\nmidground = \"ssx:track/alps0101\"").contains("only a new track"));
    let ok = parse_toml(with("music = \"ssx:music/licensed/7\"\nmidground = \"ssx:track/alps0101\"").as_bytes())
        .expect("a disc song and a disc track's mid-ground");
    assert_eq!(ok.music.map(|m| m.to_string()).as_deref(), Some("ssx:music/licensed/7"));
    assert!(parse_toml(with("music = \"pack:music/song\"").as_bytes()).is_ok(), "a package's song");
}

#[test]
fn an_object_prop_takes_no_material() {
    let object = race(|g| {
        vec![g.node(
            "{\"name\":\"p\",\"translation\":[0,0,0],\"extras\":{\"ava_type\":\"prop\",\"ava_object\":\"ssx:object/x\",\
             \"ava_material\":\"ssx:material/y\"}}",
        )]
    });
    refused(RACE, &object, "only an own mesh takes one");
    let terrain = race(|g| {
        let q = g.quad(5.0, 1.0);
        vec![g.node(&format!(
            "{{\"name\":\"t\",\"mesh\":{q},\"extras\":{{\"ava_type\":\"terrain\",\"ava_surface\":\"rock\",\
             \"ava_material\":\"ssx:material/terrain_rock_mat\"}}}}"
        ))]
    });
    let m = read(RACE, &terrain).expect("a terrain node names its material");
    let named = m.definition.terrain.iter().filter_map(|t| t.material.as_ref()).map(ToString::to_string);
    assert_eq!(named.collect::<Vec<_>>(), vec!["ssx:material/terrain_rock_mat".to_string()]);
}

#[test]
fn the_helicopter_grid_holds_each_cells_highest_terrain() {
    use super::heli::{HELI_CELL, HELI_FILL_BELOW, MAX_HELI_GRID_CELLS, MAX_HELI_GRID_VISITS, check, heli_grid};
    type Mesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);
    let def = |meshes: Vec<Mesh>| MapDefinition {
        terrain: meshes
            .into_iter()
            .map(|(positions, triangles)| TerrainMesh {
                node: "t".into(),
                weights: vec![[0.0; 8]; positions.len()],
                positions,
                triangles,
                material: None,
                texture: None,
            })
            .collect(),
        ..MapDefinition::default()
    };
    let flat = def(vec![(
        vec![[-5.0, 3.0, -5.0], [20.0, 3.0, -5.0], [20.0, 3.0, 20.0], [-5.0, 3.0, 20.0]],
        vec![[0, 3, 1], [1, 3, 2]],
    )]);
    let g = heli_grid(&flat).expect("a grid");
    assert_eq!((g.width, g.height, g.min, g.max), (3, 3, [-10.0, 3.0 - HELI_FILL_BELOW, -10.0], [20.0, 3.0, 20.0]));
    assert!(g.cells.iter().all(|c| *c == 3.0), "{:?}", g.cells);
    let tilted = def(vec![(vec![[0.0, 0.0, 0.0], [30.0, 30.0, 0.0], [0.0, 0.0, 30.0]], vec![[0, 2, 1]])]);
    let g = heli_grid(&tilted).expect("a grid");
    assert_eq!((g.width, g.height, g.min[1], g.max[1]), (3, 3, -HELI_FILL_BELOW, 30.0));
    let want = [10.0, 20.0, 30.0, 10.0, 20.0, 20.0, 10.0, 10.0, -HELI_FILL_BELOW];
    assert!(g.cells.iter().zip(want).all(|(c, w)| (c - w).abs() < 1e-4), "{:?}", g.cells);
    assert_eq!(g.cells[8], -HELI_FILL_BELOW);
    assert!(heli_grid(&MapDefinition::default()).unwrap_err().contains("the map has none"));
    let side = (MAX_HELI_GRID_CELLS as f32).sqrt() * HELI_CELL + 2.0 * HELI_CELL;
    let wide = def(vec![(vec![[0.0, 0.0, 0.0], [side, 0.0, 0.0], [0.0, 0.0, side]], vec![[0, 2, 1]])]);
    assert!(check(&wide).unwrap_err().contains(&format!("at most {MAX_HELI_GRID_CELLS} cells")));
    let half = (MAX_HELI_GRID_CELLS as f32).sqrt() * HELI_CELL / 2.0 - HELI_CELL;
    let big = vec![[0.0, 0.0, 0.0], [half, 0.0, 0.0], [0.0, 0.0, half], [half, 0.0, half]];
    let per = ((half / HELI_CELL) as u64).pow(2);
    let n = (MAX_HELI_GRID_VISITS / per + 2) as u32;
    let stacked = def(vec![(big, (0..n).map(|_| [0, 2, 3]).collect())]);
    assert!(check(&stacked).unwrap_err().contains("stacked large triangles"));
}
