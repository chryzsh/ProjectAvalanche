use gltf_json::Root;
use gltf_json::accessor::{ComponentType, Type};
use gltf_json::mesh::Semantic;
use gltf_json::validation::Checked;

use super::accessor::{F32_VEC3, F32_VEC4, Layout, read_f32, read_indices};
use super::gltf::{FileReader, with_buffers};
use super::mesh::{Budget, Transform, UV_LAYOUTS, triangles};
use super::uri::truncate;
use super::{Limits, ModelError};

pub const TERRAIN_A: &str = "AVA_TERRAIN_A";
pub const TERRAIN_B: &str = "AVA_TERRAIN_B";
const WEIGHT_LAYOUTS: [Layout; 3] =
    [F32_VEC4, (Type::Vec4, ComponentType::U8, true), (Type::Vec4, ComponentType::U16, true)];
const NAME_CHARS: usize = 128;
const PATH_CHARS: usize = 512;

fn segment(name: &str, index: usize) -> String {
    if name.is_empty() { format!("#{index}") } else { name.escape_debug().to_string() }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenePrimitive {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub local_min: [f32; 3],
    pub local_max: [f32; 3],
    pub terrain: Option<Vec<[f32; 8]>>,
    pub terrain_half: bool,
    pub material: Option<usize>,
    pub base_texture: Option<u32>,
    pub uv0: Option<Vec<[f32; 2]>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneMesh {
    pub index: usize,
    pub primitives: Vec<ScenePrimitive>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneNode {
    pub index: usize,
    pub name: String,
    pub parent: Option<usize>,
    pub xf: Transform,
    pub extras: Option<String>,
    pub mesh: Option<SceneMesh>,
    pub camera: Option<Option<f32>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub mesh_extras: Vec<Option<String>>,
    pub material_extras: Vec<Option<String>>,
}

impl Scene {
    pub fn path(&self, at: usize) -> String {
        let mut parts = Vec::new();
        let mut chars = 0;
        let mut cur = Some(at);
        while let Some(i) = cur.filter(|&i| i < self.nodes.len() && parts.len() <= self.nodes.len()) {
            let n = &self.nodes[i];
            let part = segment(&n.name, n.index);
            let len = part.chars().count() + 1;
            if !parts.is_empty() && chars + len > PATH_CHARS {
                parts.push("…".to_string());
                break;
            }
            chars += len;
            parts.push(part);
            cur = n.parent;
        }
        parts.reverse();
        parts.join("/")
    }
}

fn extras_text(raw: &gltf_json::extras::Extras, max: usize, at: &str) -> Result<Option<String>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let text = raw.get();
    if text.len() > max {
        return Err(format!("{at}.extras: {} bytes; at most {max} are read", text.len()));
    }
    Ok(Some(text.to_string()))
}

pub fn read_scene(
    rel: &str,
    bytes: &[u8],
    files: &dyn FileReader,
    limits: &Limits,
    max_extras: usize,
) -> Result<Scene, ModelError> {
    with_buffers(rel, bytes, files, limits, |root, buffers| walk(root, buffers, limits, max_extras))
}

fn walk(root: &Root, buffers: &[&[u8]], limits: &Limits, max_extras: usize) -> Result<Scene, String> {
    let scene = match (root.scene, root.scenes.len()) {
        (Some(s), _) => s.value(),
        (None, 1) => 0,
        (None, 0) => return Err("no scene".into()),
        (None, n) => return Err(format!("{n} scenes and none chosen (`scene`)")),
    };
    let mut out = Scene::default();
    for (i, m) in root.meshes.iter().enumerate() {
        out.mesh_extras.push(extras_text(&m.extras, max_extras, &format!("meshes[{i}]"))?);
    }
    for (i, m) in root.materials.iter().enumerate() {
        out.material_extras.push(extras_text(&m.extras, max_extras, &format!("materials[{i}]"))?);
    }
    let mut visited = vec![false; root.nodes.len()];
    let mut budget = Budget::default();
    let mut stack: Vec<(usize, Option<usize>, Transform, usize)> =
        root.scenes[scene].nodes.iter().rev().map(|n| (n.value(), None, Transform::IDENTITY, 1)).collect();
    while let Some((i, parent, pxf, depth)) = stack.pop() {
        if depth > limits.max_node_depth {
            return Err(format!("nodes nested deeper than {}", limits.max_node_depth));
        }
        if std::mem::replace(&mut visited[i], true) {
            return Err(format!("nodes[{i}] is reached twice (a shared node or a cycle; glTF nodes form trees)"));
        }
        let node = &root.nodes[i];
        let at = format!("nodes[{i}]");
        let xf = pxf.then(&Transform::of_node(node).map_err(|e| format!("{at}: {e}"))?);
        let extras = extras_text(&node.extras, max_extras, &at)?;
        let name: String = node.name.as_deref().unwrap_or("").chars().take(NAME_CHARS).collect();
        let mesh = match node.mesh {
            Some(m) => Some(mesh(root, buffers, m.value(), &xf, limits, &mut budget).map_err(|e| {
                let own = segment(&name, i);
                let path = parent.map_or(own.clone(), |p| format!("{}/{own}", out.path(p)));
                format!("node \"{path}\": {e}")
            })?),
            None => None,
        };
        let camera = match node.camera {
            Some(c) => {
                let cam = root.cameras.get(c.value()).ok_or_else(|| format!("{at}: camera {} missing", c.value()))?;
                Some(cam.perspective.as_ref().map(|p| p.yfov).filter(|v| v.is_finite()))
            }
            None => None,
        };
        let me = out.nodes.len();
        out.nodes.push(SceneNode { index: i, name, parent, xf, extras, mesh, camera });
        for c in node.children.iter().flatten().rev() {
            stack.push((c.value(), Some(me), xf, depth + 1));
        }
    }
    Ok(out)
}

fn mesh(
    root: &Root,
    buffers: &[&[u8]],
    m: usize,
    xf: &Transform,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<SceneMesh, String> {
    let det = xf.det3();
    if det.abs() < 1e-12 || !det.is_finite() {
        return Err(format!("meshes[{m}]: its node's transform flattens it (determinant {det:e})"));
    }
    let mut out = SceneMesh { index: m, primitives: Vec::new() };
    for (pi, p) in root.meshes[m].primitives.iter().enumerate() {
        let what = format!("meshes[{m}].primitives[{pi}]");
        let Checked::Valid(mode) = p.mode else { return Err(format!("{what}: an unknown mode")) };
        let max = limits.max_primitive_vertices;
        let attr = |s: Semantic| p.attributes.get(&Checked::Valid(s)).map(|a| a.value());
        let at = attr(Semantic::Positions).ok_or_else(|| format!("{what}: no POSITION"))?;
        let local: Vec<[f32; 3]> = read_f32(root, buffers, at, &[F32_VEC3], max)
            .map_err(|e| format!("{what}: POSITION: {e}"))?
            .0
            .as_chunks::<3>()
            .0
            .to_vec();
        let n = local.len();
        let index_list = match p.indices {
            Some(i) => read_indices(root, buffers, i.value(), n as u64, 3 * limits.max_triangles)
                .map_err(|e| format!("{what}: indices: {e}"))?,
            None => (0..n as u32).collect(),
        };
        let tris = triangles(mode, &index_list).map_err(|e| format!("{what}: {e}"))?;
        if tris.is_empty() {
            return Err(format!("{what}: no triangle"));
        }
        budget.take(n as u64, tris.len() as u64, limits).map_err(|e| format!("{what}: {e}"))?;
        let positions: Vec<[f32; 3]> = local.iter().map(|&q| xf.point(q).map(|x| x as f32)).collect();
        if positions.iter().flatten().any(|c| !c.is_finite() || c.abs() > limits.max_coordinate) {
            return Err(format!(
                "{what}: a position that is not a number within ±{} m once placed",
                limits.max_coordinate
            ));
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for q in &local {
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let weights = |name: &str| -> Result<Option<Vec<f32>>, String> {
            match attr(Semantic::Extras(name.to_string())) {
                Some(a) => {
                    let v = read_f32(root, buffers, a, &WEIGHT_LAYOUTS, max)
                        .map_err(|e| format!("{what}: _{name}: {e}"))?
                        .0;
                    if v.len() != n * 4 {
                        return Err(format!("{what}: _{name} has {} values for {n} positions", v.len() / 4));
                    }
                    if v.iter().any(|x| !(0.0..=1.0).contains(x)) {
                        return Err(format!("{what}: _{name}: a weight outside 0…1"));
                    }
                    Ok(Some(v))
                }
                None => Ok(None),
            }
        };
        let (a, b) = (weights(TERRAIN_A)?, weights(TERRAIN_B)?);
        let terrain_half = a.is_some() != b.is_some();
        let terrain = match (a, b) {
            (Some(a), Some(b)) => Some(
                (0..n).map(|v| std::array::from_fn(|k| if k < 4 { a[v * 4 + k] } else { b[v * 4 + k - 4] })).collect(),
            ),
            _ => None,
        };
        let flip = det < 0.0;
        let triangles = tris.into_iter().map(|t| if flip { [t[0], t[2], t[1]] } else { t }).collect();
        let material = p.material.map(|x| x.value());
        let base_texture = material
            .and_then(|k| root.materials.get(k))
            .and_then(|m| m.pbr_metallic_roughness.base_color_texture.as_ref())
            .map(|t| t.tex_coord);
        let uv0 = match (base_texture, attr(Semantic::TexCoords(0))) {
            (Some(_), Some(a)) => {
                let v = read_f32(root, buffers, a, &UV_LAYOUTS, max).map_err(|e| format!("{what}: TEXCOORD_0: {e}"))?.0;
                if v.len() != n * 2 {
                    return Err(format!("{what}: TEXCOORD_0 has {} values for {n} positions", v.len() / 2));
                }
                Some(v.as_chunks::<2>().0.to_vec())
            }
            _ => None,
        };
        out.primitives.push(ScenePrimitive {
            positions,
            triangles,
            local_min: lo,
            local_max: hi,
            terrain,
            terrain_half,
            material,
            base_texture,
            uv0,
        });
    }
    Ok(out)
}

pub fn shown(s: &str) -> String {
    truncate(s)
}
