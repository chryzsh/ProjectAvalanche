use gltf_json::Root;
use gltf_json::accessor::{ComponentType, Type};
use gltf_json::mesh::{Mode, Semantic};
use gltf_json::validation::Checked;

use super::accessor::{F32_VEC2, F32_VEC3, F32_VEC4, Layout, read_f32, read_indices};
use super::{Limits, Mesh, tangents};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform(pub [[f64; 4]; 4]);

impl Transform {
    pub const IDENTITY: Transform =
        Transform([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    pub fn of_node(n: &gltf_json::Node) -> Result<Transform, String> {
        if let Some(m) = n.matrix {
            if m.iter().any(|v| !v.is_finite()) {
                return Err("a matrix with a value that is not a finite number".into());
            }
            if [m[3], m[7], m[11]] != [0.0; 3] || m[15] != 1.0 {
                return Err("a matrix that is not affine".into());
            }
            return Ok(Transform(std::array::from_fn(|c| std::array::from_fn(|r| f64::from(m[c * 4 + r])))));
        }
        let t = n.translation.unwrap_or([0.0; 3]).map(f64::from);
        let s = n.scale.unwrap_or([1.0; 3]).map(f64::from);
        let [x, y, z, w] = n.rotation.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |q| q.0).map(f64::from);
        if t.iter().chain(&s).chain(&[x, y, z, w]).any(|v| !v.is_finite()) {
            return Err("a translation, rotation or scale that is not a finite number".into());
        }
        let len = (x * x + y * y + z * z + w * w).sqrt();
        if (len - 1.0).abs() > 0.01 {
            return Err(format!("a rotation of length {len:.4}, not a unit quaternion"));
        }
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let col = |c: usize| [r[c][0] * s[c], r[c][1] * s[c], r[c][2] * s[c], 0.0];
        Ok(Transform([col(0), col(1), col(2), [t[0], t[1], t[2], 1.0]]))
    }

    pub fn then(&self, child: &Transform) -> Transform {
        Transform(std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| self.0[k][r] * child.0[c][k]).sum())))
    }

    pub(super) fn det3(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2]) - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
            + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2])
    }

    pub fn point_f64(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
    }

    pub(super) fn point(&self, p: [f32; 3]) -> [f64; 3] {
        let m = &self.0;
        let p = p.map(f64::from);
        std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
    }

    fn vector(&self, v: [f32; 3]) -> [f64; 3] {
        let m = &self.0;
        let v = v.map(f64::from);
        std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
    }

    fn normal(&self, n: [f32; 3]) -> [f64; 3] {
        let m = &self.0;
        let c = |a: usize, b: usize| -> [f64; 3] {
            let (u, v) = (m[a], m[b]);
            [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        };
        let cols = [c(1, 2), c(2, 0), c(0, 1)];
        let n = n.map(f64::from);
        std::array::from_fn(|r| cols[0][r] * n[0] + cols[1][r] * n[1] + cols[2][r] * n[2])
    }
}

fn unit(v: [f64; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 && l.is_finite() { v.map(|x| (x / l) as f32) } else { [0.0; 3] }
}

#[derive(Default)]
pub struct Budget {
    pub vertices: u64,
    pub triangles: u64,
}

impl Budget {
    pub fn take(&mut self, vertices: u64, triangles: u64, limits: &Limits) -> Result<(), String> {
        self.vertices += vertices;
        self.triangles += triangles;
        if self.vertices > limits.max_vertices || self.triangles > limits.max_triangles {
            return Err(format!(
                "{} vertices and {} triangles so far; at most {} and {} are read",
                self.vertices, self.triangles, limits.max_vertices, limits.max_triangles
            ));
        }
        Ok(())
    }
}

pub(super) const UV_LAYOUTS: [Layout; 3] =
    [F32_VEC2, (Type::Vec2, ComponentType::U8, true), (Type::Vec2, ComponentType::U16, true)];
const COLOUR_LAYOUTS: [Layout; 6] = [
    F32_VEC3,
    F32_VEC4,
    (Type::Vec3, ComponentType::U8, true),
    (Type::Vec4, ComponentType::U8, true),
    (Type::Vec3, ComponentType::U16, true),
    (Type::Vec4, ComponentType::U16, true),
];

pub(super) fn triangles(mode: Mode, idx: &[u32]) -> Result<Vec<[u32; 3]>, String> {
    Ok(match mode {
        Mode::Triangles if idx.len().is_multiple_of(3) => idx.as_chunks::<3>().0.to_vec(),
        Mode::Triangles => return Err(format!("{} indices, not a multiple of 3", idx.len())),
        Mode::TriangleStrip => (0..idx.len().saturating_sub(2))
            .map(|i| if i % 2 == 0 { [idx[i], idx[i + 1], idx[i + 2]] } else { [idx[i + 1], idx[i], idx[i + 2]] })
            .collect(),
        Mode::TriangleFan => (1..idx.len().saturating_sub(1)).map(|i| [idx[i], idx[i + 1], idx[0]]).collect(),
        other => return Err(format!("mode {other:?}: only triangles are drawn")),
    })
}

pub struct Placing<'a> {
    pub xf: Transform,
    pub name: &'a str,
}

fn vec3s(v: Vec<f32>) -> Vec<[f32; 3]> {
    v.as_chunks::<3>().0.to_vec()
}

fn vec2s(v: Vec<f32>) -> Vec<[f32; 2]> {
    v.as_chunks::<2>().0.to_vec()
}

#[allow(clippy::too_many_arguments)]
pub fn primitive(
    root: &Root,
    buffers: &[&[u8]],
    (mi, pi): (usize, usize),
    at: &Placing<'_>,
    skin_joints: Option<usize>,
    limits: &Limits,
    budget: &mut Budget,
    notes: &mut Vec<String>,
) -> Result<Mesh, String> {
    let what = format!("meshes[{mi}].primitives[{pi}]");
    let p = &root.meshes[mi].primitives[pi];
    let Checked::Valid(mode) = p.mode else { return Err(format!("{what}: an unknown mode")) };
    let max = limits.max_primitive_vertices;
    let attr = |s: &Semantic| p.attributes.get(&Checked::Valid(s.clone())).map(|a| a.value());
    let positions_at = attr(&Semantic::Positions).ok_or_else(|| format!("{what}: no POSITION"))?;
    let positions = vec3s(read_f32(root, buffers, positions_at, &[F32_VEC3], max)?.0);
    let n = positions.len();
    let same = |len: usize, name: &str| -> Result<(), String> {
        if len == n { Ok(()) } else { Err(format!("{what}: {len} {name} for {n} positions")) }
    };
    let mut normals = None;
    let mut tangents4 = None;
    let (mut uv0, mut uv1, mut colours) = (None, None, None);
    for (semantic, index) in &p.attributes {
        let index = index.value();
        match semantic {
            Checked::Valid(Semantic::Positions) => {}
            Checked::Valid(Semantic::Normals) => {
                normals = Some(vec3s(read_f32(root, buffers, index, &[F32_VEC3], max)?.0))
            }
            Checked::Valid(Semantic::Tangents) => tangents4 = Some(read_f32(root, buffers, index, &[F32_VEC4], max)?.0),
            Checked::Valid(Semantic::TexCoords(0)) => {
                uv0 = Some(vec2s(read_f32(root, buffers, index, &UV_LAYOUTS, max)?.0))
            }
            Checked::Valid(Semantic::TexCoords(1)) => {
                uv1 = Some(vec2s(read_f32(root, buffers, index, &UV_LAYOUTS, max)?.0))
            }
            Checked::Valid(Semantic::Colors(0)) => {
                colours = Some(read_f32(root, buffers, index, &COLOUR_LAYOUTS, max)?)
            }
            Checked::Valid(Semantic::Joints(_) | Semantic::Weights(_)) if skin_joints.is_some() => {}
            Checked::Valid(Semantic::Joints(_) | Semantic::Weights(_)) => {
                notes.push(format!("{what}: joints and weights are not read (a static model; a rider's are)"));
            }
            Checked::Valid(s) => {
                notes.push(format!("{what}: the attribute {} is not read", ssx_core::text::quoted(&s.to_string())))
            }
            Checked::Invalid => return Err(format!("{what}: an unknown attribute")),
        }
    }
    let targets = p.targets.as_ref().map_or(0, Vec::len);
    if targets > limits.max_morph_targets {
        return Err(format!("{what}: {targets} morph targets (at most {}; none is read)", limits.max_morph_targets));
    }
    if targets > 0 {
        notes.push(format!("{what}: morph targets are not read (nothing in the game drives them)"));
    }
    let skin = match skin_joints {
        Some(nj) => Some(super::skin::read_influences(root, buffers, p, &what, n, nj, max)?),
        None => None,
    };
    if let Some(v) = &normals {
        same(v.len(), "normals")?;
    }
    if let Some(v) = &tangents4 {
        same(v.len() / 4, "tangents")?;
    }
    for (v, name) in [(&uv0, "TEXCOORD_0"), (&uv1, "TEXCOORD_1")] {
        if let Some(v) = v {
            same(v.len(), name)?;
            if v.iter().flatten().any(|c| c.abs() > limits.max_coordinate) {
                return Err(format!("{what}: a {name} beyond ±{}", limits.max_coordinate));
            }
        }
    }
    if let Some((v, k)) = &colours {
        same(v.len() / k, "colours")?;
    }
    let index_list = match p.indices {
        Some(i) => read_indices(root, buffers, i.value(), n as u64, 3 * limits.max_triangles)?,
        None => (0..n as u32).collect(),
    };
    let tris = triangles(mode, &index_list).map_err(|e| format!("{what}: {e}"))?;
    if tris.is_empty() {
        return Err(format!("{what}: no triangle"));
    }
    let det = at.xf.det3();
    if det.abs() < 1e-12 || !det.is_finite() {
        return Err(format!("{what}: its node's transform flattens it (determinant {det:e})"));
    }
    let sign = det.signum();
    let mut mesh = Mesh { name: at.name.to_string(), uv0, uv1, skin, ..Mesh::default() };
    mesh.positions = positions.iter().map(|&q| at.xf.point(q).map(|x| x as f32)).collect();
    if mesh.positions.iter().flatten().any(|c| !c.is_finite() || c.abs() > limits.max_coordinate) {
        return Err(format!("{what}: a position beyond ±{} m once placed", limits.max_coordinate));
    }
    mesh.normals = normals.map(|v| v.iter().map(|&q| unit(at.xf.normal(q).map(|x| x * sign))).collect());
    if let Some(t) = tangents4 {
        let mut dirs = Vec::with_capacity(n);
        let mut signs = Vec::with_capacity(n);
        for c in t.as_chunks::<4>().0 {
            dirs.push(unit(at.xf.vector([c[0], c[1], c[2]])));
            signs.push(if c[3] < 0.0 { -sign as f32 } else { sign as f32 });
        }
        (mesh.tangents, mesh.tangent_signs) = (Some(dirs), Some(signs));
    }
    mesh.colours = colours.map(|(v, k)| {
        v.chunks_exact(k)
            .map(|c| {
                let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                let a = if k == 4 { byte(c[3]) } else { 255 };
                [a, byte(c[2]), byte(c[1]), byte(c[0])]
            })
            .collect()
    });
    mesh.indices = tris.iter().flat_map(|t| if sign < 0.0 { [t[0], t[2], t[1]] } else { *t }).collect();
    if mesh.normals.is_none() {
        flat_normals(&mut mesh);
    }
    if mesh.tangents.is_none() && mesh.uv0.is_some() {
        let dropped = keep_used_vertices(&mut mesh);
        if dropped > 0 {
            notes.push(format!("{what}: {dropped} vertices no triangle uses left out"));
        }
        if !tangents::generate(&mut mesh) {
            notes.push(format!("{what}: MikkTSpace gave no tangents; drawn without its normal map"));
        }
    }
    budget.take(mesh.positions.len() as u64, mesh.triangle_count() as u64, limits)?;
    mesh.bounds = mesh.position_bounds();
    Ok(mesh)
}

fn flat_normals(mesh: &mut Mesh) {
    let idx = std::mem::take(&mut mesh.indices);
    let pick2 = |v: &Option<Vec<[f32; 2]>>| v.as_ref().map(|v| idx.iter().map(|&i| v[i as usize]).collect());
    let (uv0, uv1) = (pick2(&mesh.uv0), pick2(&mesh.uv1));
    let colours = mesh.colours.as_ref().map(|v| idx.iter().map(|&i| v[i as usize]).collect());
    let skin = mesh.skin.as_ref().map(|s| super::MeshSkin {
        joints: idx.iter().map(|&i| s.joints[i as usize]).collect(),
        weights: idx.iter().map(|&i| s.weights[i as usize]).collect(),
    });
    mesh.skin = skin;
    let positions: Vec<[f32; 3]> = idx.iter().map(|&i| mesh.positions[i as usize]).collect();
    let mut normals = Vec::with_capacity(positions.len());
    for t in positions.as_chunks::<3>().0 {
        let e1 = [0, 1, 2].map(|k| f64::from(t[1][k]) - f64::from(t[0][k]));
        let e2 = [0, 1, 2].map(|k| f64::from(t[2][k]) - f64::from(t[0][k]));
        let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let n = unit(c);
        let n = if n == [0.0; 3] { [0.0, 1.0, 0.0] } else { n };
        normals.extend([n; 3]);
    }
    mesh.indices = (0..positions.len() as u32).collect();
    (mesh.positions, mesh.normals, mesh.uv0, mesh.uv1, mesh.colours) = (positions, Some(normals), uv0, uv1, colours);
    mesh.tangents = None;
    mesh.tangent_signs = None;
}

pub(super) fn keep_used_vertices(mesh: &mut Mesh) -> usize {
    let n = mesh.positions.len();
    let mut used = vec![false; n];
    for &i in &mesh.indices {
        used[i as usize] = true;
    }
    let kept = used.iter().filter(|u| **u).count();
    if kept == n {
        return 0;
    }
    let mut to = vec![0u32; n];
    let mut next = 0u32;
    for (v, &u) in used.iter().enumerate() {
        if u {
            to[v] = next;
            next += 1;
        }
    }
    fn pick<T: Copy>(v: &mut Vec<T>, used: &[bool]) {
        let mut k = 0;
        v.retain(|_| {
            k += 1;
            used[k - 1]
        });
    }
    pick(&mut mesh.positions, &used);
    if let Some(a) = mesh.normals.as_mut() {
        pick(a, &used);
    }
    if let Some(a) = mesh.tangents.as_mut() {
        pick(a, &used);
    }
    if let Some(a) = mesh.tangent_signs.as_mut() {
        pick(a, &used);
    }
    if let Some(a) = mesh.uv0.as_mut() {
        pick(a, &used);
    }
    if let Some(a) = mesh.uv1.as_mut() {
        pick(a, &used);
    }
    if let Some(a) = mesh.colours.as_mut() {
        pick(a, &used);
    }
    if let Some(s) = mesh.skin.as_mut() {
        pick(&mut s.joints, &used);
        pick(&mut s.weights, &used);
    }
    for i in &mut mesh.indices {
        *i = to[*i as usize];
    }
    n - kept
}
