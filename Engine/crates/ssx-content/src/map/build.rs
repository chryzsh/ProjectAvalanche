use std::collections::BTreeMap;

use serde_json::Value;

use super::volume::{mesh_box, placement, sphere};
use super::*;
use crate::model::collision::{GROUPS, surface_id};
use crate::model::scene::{Scene, SceneNode, read_scene};
use crate::model::{FileReader, ModelError};

const TYPES: [&str; 13] = [
    "terrain",
    "prop",
    "collision",
    "rail",
    "spawn",
    "checkpoint",
    "finish",
    "trigger",
    "death",
    "boundary",
    "marker",
    "heli_path",
    "camera",
];

#[derive(Default)]
struct Ava {
    ty: Option<String>,
    surface: Option<String>,
    group: Option<u32>,
    hidden: Option<bool>,
    slot: Option<u32>,
    radius: Option<f32>,
    order: Option<u32>,
    name: Option<String>,
    object: Option<ContentId>,
    marker: Option<ContentId>,
    ao: Option<f32>,
    material: Option<ContentId>,
    keys: Vec<&'static str>,
}

const KEYS: [&str; 12] = [
    "ava_type",
    "ava_surface",
    "ava_group",
    "ava_hidden",
    "ava_slot",
    "ava_radius",
    "ava_order",
    "ava_name",
    "ava_object",
    "ava_marker",
    "ava_ao",
    "ava_material",
];

fn name_of(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) if !s.is_empty() && s.len() <= MAX_NAME_BYTES && !s.chars().any(char::is_control) => {
            Ok(s.clone())
        }
        _ => Err(format!("1 to {MAX_NAME_BYTES} bytes of text without control characters")),
    }
}

fn id_value(v: &Value, kind: crate::id::Kind) -> Result<ContentId, String> {
    let Value::String(s) = v else { return Err(format!("a {kind} id as text")) };
    let id: ContentId = s.parse().map_err(|e| format!("{e}"))?;
    if id.kind() != kind {
        return Err(format!("{id} is not a {kind} id"));
    }
    Ok(id)
}

fn float_in(v: &Value, lo: f32, hi: f32) -> Result<f32, String> {
    match v.as_f64() {
        Some(x) if x.is_finite() && (f64::from(lo)..=f64::from(hi)).contains(&x) => Ok(x as f32),
        _ => Err(format!("a number from {lo} to {hi}")),
    }
}

fn int_below(v: &Value, n: u32) -> Result<u32, String> {
    match v.as_u64() {
        Some(x) if x < u64::from(n) => Ok(x as u32),
        _ => Err(format!("a whole number from 0 to {}", n - 1)),
    }
}

fn ava(text: Option<&str>) -> Result<Ava, (String, String)> {
    let mut a = Ava::default();
    let Some(text) = text else { return Ok(a) };
    let value: Value = serde_json::from_str(text)
        .map_err(|e| ("extras".to_string(), ssx_core::text::message(&e.to_string()).into_string()))?;
    let Value::Object(map) = value else { return Ok(a) };
    for (k, v) in &map {
        if !k.starts_with("ava_") {
            continue;
        }
        let Some(&key) = KEYS.iter().find(|x| **x == k.as_str()) else {
            let shown = crate::model::scene::shown(k);
            return Err((shown, format!("not one of the map's keys {}", KEYS.join(", "))));
        };
        let field = |e: String| (key.to_string(), e);
        match key {
            "ava_type" => match v {
                Value::String(s) if TYPES.contains(&s.as_str()) => a.ty = Some(s.clone()),
                _ => return Err(field(format!("one of {}", TYPES.join(", ")))),
            },
            "ava_surface" => match v {
                Value::String(s) if s.len() <= MAX_NAME_BYTES => a.surface = Some(s.clone()),
                _ => return Err(field("a surface name".into())),
            },
            "ava_group" => match v.as_u64() {
                Some(g) if GROUPS.contains(&(g as u32)) && g <= u64::from(u32::MAX) => a.group = Some(g as u32),
                _ => return Err(field(format!("one of {GROUPS:?}"))),
            },
            "ava_hidden" => a.hidden = Some(v.as_bool().ok_or_else(|| field("true or false".into()))?),
            "ava_slot" => a.slot = Some(int_below(v, MAX_SPAWNS as u32).map_err(field)?),
            "ava_radius" => a.radius = Some(float_in(v, 0.01, MAX_COORDINATE).map_err(field)?),
            "ava_order" => a.order = Some(int_below(v, MAX_ORDER).map_err(field)?),
            "ava_name" => a.name = Some(name_of(v).map_err(field)?),
            "ava_object" => a.object = Some(id_value(v, crate::id::Kind::Object).map_err(field)?),
            "ava_marker" => a.marker = Some(id_value(v, crate::id::Kind::Marker).map_err(field)?),
            "ava_ao" => a.ao = Some(float_in(v, 0.0, MAX_AO).map_err(field)?),
            "ava_material" => a.material = Some(id_value(v, crate::id::Kind::Material).map_err(field)?),
            _ => {}
        }
        a.keys.push(key);
    }
    Ok(a)
}

fn reads(ty: &str) -> &'static [&'static str] {
    match ty {
        "terrain" => &["ava_surface", "ava_material"],
        "prop" => &["ava_object", "ava_ao", "ava_material"],
        "collision" => &["ava_surface", "ava_group"],
        "rail" => &["ava_surface", "ava_hidden", "ava_material"],
        "spawn" => &["ava_slot"],
        "checkpoint" => &["ava_order", "ava_radius"],
        "finish" | "death" => &["ava_radius"],
        "trigger" => &["ava_name", "ava_radius"],
        "marker" => &["ava_marker"],
        "heli_path" => &["ava_order"],
        "camera" => &["ava_name"],
        _ => &[],
    }
}

pub fn build(scene: &Scene, toml: &MapToml, file: &str) -> Result<MapDefinition, MapError> {
    let mut b = Builder {
        scene,
        file,
        def: MapDefinition::default(),
        slots: BTreeMap::new(),
        orders: BTreeMap::new(),
        heli: BTreeMap::new(),
        unused_cameras: (0, None),
    };
    for (what, list) in [("meshes", &scene.mesh_extras), ("materials", &scene.material_extras)] {
        for (k, text) in list.iter().enumerate() {
            let a = ava(text.as_deref()).map_err(|(f, m)| MapError::new(file, format!("{what}[{k}] {f}"), m))?;
            if let Some(key) = a.keys.iter().find(|k| **k != "ava_surface") {
                return Err(MapError::new(
                    file,
                    format!("{what}[{k}] {key}"),
                    "a mesh or material carries only ava_surface",
                ));
            }
        }
    }
    for i in 0..scene.nodes.len() {
        b.node(i)?;
    }
    b.finish(toml)
}

struct Builder<'s> {
    scene: &'s Scene,
    file: &'s str,
    def: MapDefinition,
    slots: BTreeMap<u32, Spawn>,
    orders: BTreeMap<u32, Volume>,
    heli: BTreeMap<u32, HeliPoint>,
    unused_cameras: (usize, Option<usize>),
}

impl Builder<'_> {
    fn err(&self, i: usize, field: &str, message: impl Into<String>) -> MapError {
        let path = self.scene.path(i);
        let at = if field.is_empty() { format!("node \"{path}\"") } else { format!("node \"{path}\" {field}") };
        MapError::new(self.file, at, message)
    }

    fn surface(&self, n: &SceneNode, own: &Option<String>) -> Result<Option<String>, (String, String)> {
        if own.is_some() {
            return Ok(own.clone());
        }
        let Some(m) = &n.mesh else { return Ok(None) };
        let mesh = ava(self.scene.mesh_extras.get(m.index).and_then(|x| x.as_deref()))?;
        if mesh.surface.is_some() {
            return Ok(mesh.surface);
        }
        for p in &m.primitives {
            if let Some(k) = p.material {
                let mat = ava(self.scene.material_extras.get(k).and_then(|x| x.as_deref()))?;
                if mat.surface.is_some() {
                    return Ok(mat.surface);
                }
            }
        }
        Ok(None)
    }

    fn node(&mut self, i: usize) -> Result<(), MapError> {
        let n = &self.scene.nodes[i];
        let a = ava(n.extras.as_deref()).map_err(|(f, m)| self.err(i, &f, m))?;
        let Some(ty) = a.ty.clone() else {
            if n.mesh.is_some() {
                return Err(self.err(
                    i,
                    "ava_type",
                    "a mesh without ava_type: say what it is (terrain, prop, collision, …)",
                ));
            }
            if n.camera.is_some() {
                self.unused_cameras.0 += 1;
                self.unused_cameras.1.get_or_insert(i);
            }
            if let Some(k) = a.keys.first() {
                return Err(self.err(i, k, "a node without ava_type takes no ava_* keys"));
            }
            return Ok(());
        };
        if let Some(k) = a.keys.iter().find(|k| **k != "ava_type" && !reads(&ty).contains(k)) {
            return Err(self.err(i, k, format!("not read on a {ty} node")));
        }
        let path = self.scene.path(i);
        let needs_mesh = matches!(ty.as_str(), "terrain" | "collision" | "rail" | "boundary");
        let no_mesh = matches!(ty.as_str(), "spawn" | "marker" | "heli_path" | "camera");
        if needs_mesh && n.mesh.is_none() {
            return Err(self.err(i, "ava_type", format!("a {ty} node has a mesh")));
        }
        if no_mesh && n.mesh.is_some() {
            return Err(self.err(i, "ava_type", format!("a {ty} node is an empty, without a mesh")));
        }
        let place = |b: &Self| placement(&n.xf).map_err(|e| b.err(i, "", e));
        match ty.as_str() {
            "terrain" => self.terrain(i, n, &a, path)?,
            "collision" | "rail" => self.collision(i, n, &a, &ty, path)?,
            "boundary" => {
                let area = self.def.boundary.get_or_insert_with(BoundaryArea::default);
                for p in n.mesh.iter().flat_map(|m| &m.primitives) {
                    for t in &p.triangles {
                        area.triangles.push(t.map(|v| [p.positions[v as usize][0], p.positions[v as usize][2]]));
                    }
                }
            }
            "prop" => {
                let look = match (&n.mesh, &a.object) {
                    (Some(_), Some(_)) => {
                        return Err(self.err(i, "ava_object", "a prop is its own mesh or an object id, not both"));
                    }
                    (None, None) => {
                        return Err(self.err(i, "ava_object", "an empty prop names an object (ava_object)"));
                    }
                    (Some(m), None) => PropLook::Mesh {
                        vertices: m.primitives.iter().map(|p| p.positions.len()).sum(),
                        triangles: m.primitives.iter().map(|p| p.triangles.len()).sum(),
                    },
                    (None, Some(id)) => PropLook::Object(id.clone()),
                };
                if a.material.is_some() && matches!(look, PropLook::Object(_)) {
                    return Err(self.err(
                        i,
                        "ava_material",
                        "an object's materials are its record's (only an own mesh takes one)",
                    ));
                }
                let placement = place(self)?;
                let (ao, gltf_node, material) = (a.ao.unwrap_or(1.0), n.index, a.material.clone());
                self.def.props.push(Prop { node: path, look, placement, ao, gltf_node, material });
            }
            "spawn" => {
                let slot =
                    a.slot.ok_or_else(|| self.err(i, "ava_slot", "a spawn has its slot (0 … 9; 0 the player's)"))?;
                let p = place(self)?;
                if self.slots.contains_key(&slot) {
                    return Err(self.err(i, "ava_slot", format!("slot {slot} twice")));
                }
                self.slots.insert(slot, Spawn { node: path, slot, position: p.position, rotation: p.rotation });
            }
            "checkpoint" | "finish" | "trigger" | "death" => self.volume(i, n, &a, &ty, path)?,
            "marker" => {
                let marker =
                    a.marker.clone().ok_or_else(|| self.err(i, "ava_marker", "a marker names its marker id"))?;
                if self.def.markers.len() >= MAX_MARKERS {
                    return Err(self.err(i, "", format!("more than {MAX_MARKERS} markers")));
                }
                let placement = place(self)?;
                self.def.markers.push(MarkerPlacement { node: path, marker, placement });
            }
            "heli_path" => {
                let order = a.order.ok_or_else(|| {
                    self.err(
                        i,
                        "ava_order",
                        "a heli_path point has its order (0 the intro locator, 1 the outro locator)",
                    )
                })?;
                if order as usize >= HELI_PATH_POINTS {
                    return Err(self.err(
                        i,
                        "ava_order",
                        "0 (the intro locator) or 1 (the outro locator): the helicopter's path is its two locators",
                    ));
                }
                let p = place(self)?;
                if self.heli.contains_key(&order) {
                    return Err(self.err(i, "ava_order", format!("order {order} twice")));
                }
                self.heli.insert(order, HeliPoint { node: path, order, position: p.position, rotation: p.rotation });
            }
            "camera" => {
                let Some(yfov) = n.camera else { return Err(self.err(i, "ava_type", "a camera node has a camera")) };
                if self.def.cameras.len() >= MAX_CAMERAS {
                    return Err(self.err(i, "", format!("more than {MAX_CAMERAS} cameras")));
                }
                let p = place(self)?;
                let name = a.name.clone().unwrap_or_default();
                self.def.cameras.push(CameraDef { node: path, name, position: p.position, rotation: p.rotation, yfov });
            }
            _ => {}
        }
        Ok(())
    }
}

impl Builder<'_> {
    fn terrain(&mut self, i: usize, n: &SceneNode, a: &Ava, path: String) -> Result<(), MapError> {
        let surface = self.surface(n, &a.surface).map_err(|(f, m)| self.err(i, &f, m))?;
        let constant = match &surface {
            Some(s) => Some(TERRAIN_SURFACES.iter().find(|(k, _)| k == s).map(|(_, w)| *w).ok_or_else(|| {
                let names: Vec<&str> = TERRAIN_SURFACES.iter().map(|(k, _)| *k).collect();
                self.err(
                    i,
                    "ava_surface",
                    format!("{} is not a terrain surface ({})", ssx_core::text::quoted_max(s, 128), names.join(", ")),
                )
            })?),
            None => None,
        };
        for p in n.mesh.iter().flat_map(|m| &m.primitives) {
            let texture = match (p.base_texture, p.material, &p.uv0) {
                (Some(0), Some(material), Some(uv)) => Some(TerrainTexture { material, uv: uv.clone() }),
                (Some(0), _, None) => {
                    return Err(self.err(
                        i,
                        "TEXCOORD_0",
                        "its material has a base colour texture, but the mesh has no TEXCOORD_0 to place it with",
                    ));
                }
                (Some(c), ..) => {
                    return Err(self.err(
                        i,
                        "",
                        format!("its material's base colour texture reads TEXCOORD_{c}; a terrain's texture is placed by TEXCOORD_0"),
                    ));
                }
                (None, ..) => None,
            };
            if p.terrain_half {
                return Err(self.err(
                    i,
                    "_AVA_TERRAIN_A",
                    "a terrain mesh has both _AVA_TERRAIN_A and _AVA_TERRAIN_B, or neither",
                ));
            }
            let weights = match (&p.terrain, constant) {
                (Some(_), Some(_)) => {
                    return Err(self.err(i, "ava_surface", "per-vertex terrain weights or ava_surface, not both"));
                }
                (Some(w), None) => w.clone(),
                (None, Some(c)) => vec![c; p.positions.len()],
                (None, None) => {
                    return Err(self.err(
                        i,
                        "ava_surface",
                        "a terrain mesh has ava_surface or the weights _AVA_TERRAIN_A / _B",
                    ));
                }
            };
            self.count(i, p.triangles.len())?;
            self.def.terrain.push(TerrainMesh {
                node: path.clone(),
                positions: p.positions.clone(),
                triangles: p.triangles.clone(),
                weights,
                material: a.material.clone(),
                texture,
            });
        }
        Ok(())
    }

    fn count(&self, i: usize, more: usize) -> Result<(), MapError> {
        let n = self.def.collision_triangles() + more;
        if n > MAX_COLLISION_TRIANGLES {
            return Err(self.err(
                i,
                "",
                format!("{n} collision triangles; at most {MAX_COLLISION_TRIANGLES} (the disc's largest drop world)"),
            ));
        }
        Ok(())
    }

    fn collision(&mut self, i: usize, n: &SceneNode, a: &Ava, ty: &str, path: String) -> Result<(), MapError> {
        let surface = self.surface(n, &a.surface).map_err(|(f, m)| self.err(i, &f, m))?;
        let surface = surface.ok_or_else(|| {
            self.err(i, "ava_surface", format!("a {ty} mesh has ava_surface (on its node, mesh or material)"))
        })?;
        let surface_id = surface_id(&surface).map_err(|e| self.err(i, "ava_surface", e))?;
        let (group, hidden) =
            if ty == "rail" { (RAIL_GROUP, a.hidden.unwrap_or(false)) } else { (a.group.unwrap_or(0), true) };
        let mut triangles = Vec::new();
        for p in n.mesh.iter().flat_map(|m| &m.primitives) {
            self.count(i, triangles.len() + p.triangles.len())?;
            for t in &p.triangles {
                let corners = t.map(|v| p.positions[v as usize]);
                triangles.push(CollisionTriangle { corners, surface_id, group });
            }
        }
        let material = if ty == "rail" { a.material.clone() } else { None };
        let mesh = CollisionMesh { node: path, triangles, hidden, material };
        if ty == "rail" {
            self.def.rails.push(mesh)
        } else {
            self.def.collision.push(mesh)
        }
        Ok(())
    }

    fn volume(&mut self, i: usize, n: &SceneNode, a: &Ava, ty: &str, path: String) -> Result<(), MapError> {
        let shape = match (&n.mesh, a.radius) {
            (Some(_), Some(_)) => {
                return Err(self.err(i, "ava_radius", "a volume is a mesh's box or an empty's sphere, not both"));
            }
            (Some(m), None) => {
                let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
                for p in &m.primitives {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p.local_min[k]);
                        hi[k] = hi[k].max(p.local_max[k]);
                    }
                }
                mesh_box(&n.xf, lo, hi).map_err(|e| self.err(i, "", e))?
            }
            (None, Some(r)) => sphere(&n.xf, r).map_err(|e| self.err(i, "ava_radius", e))?,
            (None, None) => {
                return Err(self.err(
                    i,
                    "ava_radius",
                    format!("a {ty} is a mesh (its box) or an empty with ava_radius"),
                ));
            }
        };
        if self.def.volume_count() + self.orders.len() >= MAX_VOLUMES {
            return Err(self.err(i, "", format!("more than {MAX_VOLUMES} volumes")));
        }
        let name = match ty {
            "trigger" => {
                let name =
                    a.name.clone().ok_or_else(|| self.err(i, "ava_name", "a trigger has its name (ava_name)"))?;
                if name == "end" {
                    return Err(self.err(i, "ava_name", "\"end\" is the finish's name; a finish node makes it"));
                }
                name
            }
            "finish" => "end".to_string(),
            other => other.to_string(),
        };
        let order = a.order.unwrap_or(0);
        let v = Volume { node: path, name, order, shape };
        match ty {
            "checkpoint" => {
                let order =
                    a.order.ok_or_else(|| self.err(i, "ava_order", "a checkpoint has its order (ava_order)"))?;
                if order as usize >= MAX_CHECKPOINTS {
                    return Err(self.err(
                        i,
                        "ava_order",
                        format!("0 to {} (the game's check list holds {MAX_CHECKPOINTS})", MAX_CHECKPOINTS - 1),
                    ));
                }
                if self.orders.insert(order, v).is_some() {
                    return Err(self.err(i, "ava_order", format!("order {order} twice")));
                }
            }
            "finish" if self.def.finish.is_some() => return Err(self.err(i, "ava_type", "a second finish")),
            "finish" => self.def.finish = Some(v),
            "trigger" => self.def.triggers.push(v),
            _ => self.def.deaths.push(v),
        }
        Ok(())
    }

    fn finish(mut self, toml: &MapToml) -> Result<MapDefinition, MapError> {
        let whole = |m: String| MapError::new(self.file, "", m);
        if let (n, Some(first)) = self.unused_cameras {
            let path = self.scene.path(first);
            self.def.notes.push(match n {
                1 => format!("node \"{path}\": a camera without ava_type is not used"),
                n => format!("{n} cameras without ava_type are not used (the first: node \"{path}\")"),
            });
        }
        self.def.spawns = std::mem::take(&mut self.slots).into_values().collect();
        self.def.checkpoints = std::mem::take(&mut self.orders).into_values().collect();
        self.def.heli_path = std::mem::take(&mut self.heli).into_values().collect();
        let layout = toml.base.is_some();
        if layout {
            if let Some(t) = self.def.terrain.first() {
                return Err(MapError::new(
                    self.file,
                    format!("node \"{}\"", t.node),
                    "a layout on a disc drop adds no terrain: its drop's terrain stays (use collision or props)",
                ));
            }
            if self.def.boundary.is_some() {
                return Err(whole("a layout on a disc drop keeps its drop's boundary; it carries none".into()));
            }
        } else {
            if self.def.spawns.is_empty() {
                return Err(whole("a new track has a spawn (an empty with ava_type \"spawn\", ava_slot 0)".into()));
            }
            if let Some(m) = toml.modes.iter().find(|m| m.needs_finish())
                && self.def.finish.is_none()
            {
                return Err(whole(format!(
                    "a {} map needs a finish (a node with ava_type \"finish\": a box mesh, or an empty with ava_radius)",
                    m.name()
                )));
            }
            if self.def.terrain.is_empty() && self.def.collision.is_empty() {
                return Err(whole("a new track has ground: a terrain or collision mesh".into()));
            }
        }
        if let Some(s) = self.def.spawns.first().filter(|s| s.slot != 0) {
            return Err(MapError::new(
                self.file,
                format!("node \"{}\" ava_slot", s.node),
                "slot 0, the player's, comes first",
            ));
        }
        if let Some((k, s)) = self.def.spawns.iter().enumerate().find(|(k, s)| s.slot as usize != *k) {
            let message = format!("the spawns' slots run 0, 1, 2 … without a gap; slot {k} is missing");
            return Err(MapError::new(self.file, format!("node \"{}\" ava_slot", s.node), message));
        }
        if self.def.heli_path.len() == 1 {
            let p = &self.def.heli_path[0];
            let missing = 1 - p.order;
            return Err(MapError::new(
                self.file,
                format!("node \"{}\" ava_order", p.node),
                format!(
                    "the heli_path has both locators: order {missing} ({}) is missing",
                    HELI_LOCATORS[missing as usize]
                ),
            ));
        }
        if toml.intro == Intro::Heli && !layout {
            super::heli::check(&self.def).map_err(whole)?;
        }
        if toml.intro == Intro::Heli && !layout && self.def.heli_path.is_empty() {
            return Err(whole(
                "intro = \"heli\" on a new track needs a heli_path: two empties with ava_type \"heli_path\", \
                 ava_order 0 (the intro locator) and 1 (the outro locator, where the helicopter waits at the end)"
                    .into(),
            ));
        }
        if !self.def.heli_path.is_empty() && toml.intro != Intro::Heli {
            self.def.notes.push("the heli_path is flown with intro = \"heli\" (this map starts at its spawn)".into());
        }
        if let Some(b) = self.def.boundary.as_mut() {
            b.raster = super::boundary::raster(b).map_err(whole)?;
        }
        Ok(self.def)
    }
}

pub fn read_map(toml_bytes: &[u8], toml_file: &str, files: &dyn FileReader) -> Result<Map, MapError> {
    let toml = parse_toml(toml_bytes).map_err(|p| {
        let (line, column, m) = p.into_iter().next().unwrap_or((0, 0, "unreadable".into()));
        MapError::new(toml_file, format!("{line}:{column}"), m)
    })?;
    let limits = scene_limits();
    let bytes = files.read(&toml.scene, limits.max_file_bytes).map_err(|e| MapError::new(toml_file, "scene", e))?;
    let scene = read_scene(&toml.scene, &bytes, files, &limits, MAX_EXTRAS_BYTES)
        .map_err(|ModelError { file, message }| MapError::new(&file, "", message))?;
    let definition = build(&scene, &toml, &toml.scene)?;
    let nodes: Vec<usize> =
        definition.props.iter().filter(|p| matches!(p.look, PropLook::Mesh { .. })).map(|p| p.gltf_node).collect();
    let textured = definition.terrain.iter().any(|t| t.texture.is_some());
    let props = if nodes.is_empty() && !textured {
        None
    } else {
        let (model, ranges, cost) = crate::model::decode_node_meshes(&toml.scene, &bytes, files, &limits, &nodes)
            .map_err(|ModelError { file, message }| MapError::new(&file, "props", message))?;
        Some(PropMeshes { model: std::sync::Arc::new(model), ranges, cost })
    };
    Ok(Map { toml, definition, props })
}
