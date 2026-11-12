import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    mesh_view_bindings as view_bindings,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
};
import package::render::lighting::game_lighting as gl;

struct TerrainParams {
    base: array<vec4<f32>, 2>,
    snow_tile: array<vec4<f32>, 2>,
    snow_eccentricity: array<vec4<f32>, 2>,
    snow_specular: array<vec4<f32>, 2>,
    planar_scatter: vec4<f32>,
    snow_blend: vec4<f32>,
    snow_fall: vec4<f32>,
    reflect: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> terrain: TerrainParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var rock_diffuse_1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var rock_normal_1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(111) var rock_diffuse_2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(112) var rock_normal_2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var snow_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var terrain_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var scatter_normal: texture_2d<f32>;
@if(SSX_LOCAL_LIGHTS)
var<private> light: gl::GameLight;
@else
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var<uniform> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var reflect_cube: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var cube_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var sparkle_normal: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var sparkle_sampler: sampler;
struct SnowTracks {
    tile: vec4<f32>,
    size: vec4<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(113) var<uniform> tracks: SnowTracks;
@group(constants::MATERIAL_BIND_GROUP) @binding(114) var track_lookup: texture_2d<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(115) var track_tiles: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(116) var track_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(118) var base_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(119) var base_sampler: sampler;

const NO_LAYER: u32 = 7u;
const POWDER: u32 = 5u;
const TEXTURE_COVERAGE_SHIFT: u32 = 14u;
const FADE: vec4<f32> = vec4(0.02, 0.025, 2.5, 12.0);
const SNOW_ALBEDO: f32 = 0.5;
fn snow_tile(i: u32) -> f32 { return terrain.snow_tile[i / 4u][i % 4u]; }
fn snow_eccentricity(i: u32) -> f32 { return terrain.snow_eccentricity[i / 4u][i % 4u]; }
fn snow_specular(i: u32) -> f32 { return terrain.snow_specular[i / 4u][i % 4u]; }

struct Surface {
    p: vec3<f32>,
    dx: vec3<f32>,
    dy: vec3<f32>,
}

fn plane_uv(v: vec3<f32>, axis: u32) -> vec2<f32> {
    switch axis {
        case 0u: { return vec2(v.z, -v.y); }
        case 1u: { return vec2(v.x, v.z); }
        default: { return vec2(v.x, -v.y); }
    }
}

fn sample_plane(tex: texture_2d<f32>, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    return textureSampleGrad(
        tex, terrain_sampler, plane_uv(s.p, axis) * tile,
        plane_uv(s.dx, axis) * tile, plane_uv(s.dy, axis) * tile,
    );
}

fn sample_base_normal(k: u32, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    if k == 0u {
        return sample_plane(rock_normal_1, s, axis, tile);
    }
    return sample_plane(rock_normal_2, s, axis, tile);
}

fn sample_base_diffuse(k: u32, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    if k == 0u {
        return sample_plane(rock_diffuse_1, s, axis, tile);
    }
    return sample_plane(rock_diffuse_2, s, axis, tile);
}

fn planar_weights(n: vec3<f32>) -> vec3<f32> {
    let w = max(abs(n) * terrain.planar_scatter.x + terrain.planar_scatter.y, vec3(0.0));
    return w / max(w.x + w.y + w.z, 1e-6);
}

fn base_perturbation(k: u32, s: Surface, w: vec3<f32>) -> vec3<f32> {
    let tile = terrain.base[k].x;
    var d = vec3(0.0);
    if w.x > 0.0 {
        let t = sample_base_normal(k, s, 0u, tile).xy * 2.0 - 1.0;
        d += w.x * vec3(0.0, t.y, t.x);
    }
    if w.y > 0.0 {
        let t = sample_base_normal(k, s, 1u, tile).xy * 2.0 - 1.0;
        d += w.y * vec3(t.y, 0.0, t.x);
    }
    if w.z > 0.0 {
        let t = sample_base_normal(k, s, 2u, tile).xy * 2.0 - 1.0;
        d += w.z * vec3(t.x, t.y, 0.0);
    }
    return d;
}

fn base_diffuse(k: u32, s: Surface, w: vec3<f32>) -> vec4<f32> {
    let tile = terrain.base[k].x;
    var sum = vec4(0.0);
    for (var axis = 0u; axis < 3u; axis++) {
        if w[axis] > 0.0 {
            sum += w[axis] * sample_base_diffuse(k, s, axis, tile);
        }
    }
    return sum;
}

fn deformation(n: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let tile_uv = p.xz * tracks.tile.x + tracks.tile.yz;
    let size = vec2<i32>(tracks.size.xy);
    let idx = textureLoad(track_lookup, clamp(vec2<i32>(floor(tile_uv)), vec2(0), size - 1), 0).x;
    if idx >= 65535u {
        return vec3(0.0, 0.0, 1.0);
    }
    let d = textureSampleLevel(track_tiles, track_sampler, fract(tile_uv), i32(idx), 0.0);
    let f = saturate(4.0 * n.y - 0.6);
    return vec3(2.0 * (d.xy - 0.5) * f, saturate(1.0 - 100.0 * abs((d.w - 0.75) * f)));
}

fn snow_detail(n: vec3<f32>, s: Surface, w: vec4<f32>, lanes_in: array<u32, 4>) -> vec3<f32> {
    var lanes = lanes_in;
    var t = vec2(0.0);
    for (var i = 0u; i < 4u; i++) {
        let layer = lanes[i];
        if layer != NO_LAYER && w[i] != 0.0 {
            let tile = snow_tile(layer);
            let m = textureSampleGrad(snow_normal, terrain_sampler, s.p.xz * tile, layer, s.dx.xz * tile, s.dy.xz * tile);
            t += w[i] * (m.xy * 2.0 - 1.0);
        }
    }
    let st = terrain.planar_scatter.z;
    let gb = st * exp2(terrain.reflect.z);
    let scatter = textureSampleGrad(scatter_normal, terrain_sampler, s.p.xz * st, s.dx.xz * gb, s.dy.xz * gb);
    let sc = terrain.planar_scatter.w * (scatter.xyz * 2.0 - 1.0);
    let dm = deformation(n, s.p);
    return normalize(vec3(n.x + dm.x + dm.z * t.y + sc.x, n.y + sc.y, n.z + dm.y + dm.z * t.x + sc.z));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
@if(SSX_LOCAL_LIGHTS)
    light = gl::frame_light();
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    var s: Surface;
    s.p = in.world_position.xyz;
    s.dx = dpdx(s.p);
    s.dy = dpdy(s.p);

    let w_snow = in.color;
    let textured = terrain.reflect.w > 0.5;
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    var cov = 2.0 * in.uv.x - 1.0;
    let code = u32(round(in.uv_b.y));
    let group = u32(round(terrain.snow_fall.w));
    var base_bits = (code >> 12u) & 3u;
    if group == 1u {
        base_bits = 0u;
    }
    var lanes: array<u32, 4>;
    var has_snow = false;
    for (var i = 0u; i < 4u; i++) {
        lanes[i] = (code >> (3u * i)) & 7u;
        has_snow = has_snow || lanes[i] != NO_LAYER;
    }
    if group == 2u {
        has_snow = false;
    }
    var b = in.uv.y;
    if base_bits == 1u {
        b = 0.0;
    } else if base_bits == 2u {
        b = 1.0;
    }
    if textured {
        cov = f32((code >> TEXTURE_COVERAGE_SHIFT) & 255u) * (2.0 / 255.0) - 1.0;
        b = 0.0;
    }

    let wp = vec4(in.world_position.xyz, 1.0);
    let view_z = dot(vec4(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z,
    ), wp);
    var fade_snow = saturate(view_z * FADE.x + FADE.z);
    var fade_base = saturate(view_z * FADE.y + FADE.w);
    if terrain.snow_blend.w == 0.0 {
        fade_snow = 1.0;
        fade_base = 1.0;
    }

    let n = normalize(in.world_normal);
    var n_base = n;
    var ecc_base = 1.0;
    if base_bits != 0u {
        ecc_base = mix(terrain.base[0].z, terrain.base[1].z, b);
        if fade_base > 0.0 {
            let w = planar_weights(n);
            var d = vec3(0.0);
            if b != 1.0 { d += (1.0 - b) * terrain.base[0].y * base_perturbation(0u, s, w); }
            if b != 0.0 { d += b * terrain.base[1].y * base_perturbation(1u, s, w); }
            n_base = mix(n, normalize(n + d), fade_base);
        }
    }
    var normal = n_base;
    var ecc = ecc_base;
    var snow_spec_types = 0.0;
    if has_snow {
        var ecc_types = 0.0;
        for (var i = 0u; i < 4u; i++) {
            var ecc_i = 1.0;
            var spec_i = 1.0;
            if lanes[i] != NO_LAYER {
                ecc_i = snow_eccentricity(lanes[i]);
                spec_i = snow_specular(lanes[i]);
            }
            ecc_types += w_snow[i] * ecc_i;
            snow_spec_types += w_snow[i] * spec_i;
        }
        let ecc_snow = mix(snow_eccentricity(POWDER), ecc_types, fade_snow);
        var n_snow = n;
        if fade_snow > 0.0 {
            n_snow = mix(n, snow_detail(n, s, w_snow, lanes), fade_snow);
        }
        let c = saturate(cov);
        normal = mix(n_base, normalize(n_snow), c);
        ecc = mix(ecc_base, ecc_snow, c);
    }
    normal = normalize(normal);

    let sb = terrain.snow_blend;
    let fall = dot(normal, -normalize(terrain.snow_fall.xyz));
    let x = saturate(cov + sb.x + fall);
    let t = saturate((x - (sb.y - sb.z)) / max(2.0 * sb.z, 1e-6));
    var cvg = mix(1.0, t * t * (3.0 - 2.0 * t), fade_base);
    if base_bits == 0u {
        cvg = 1.0;
    }
    let snow_spec = mix(snow_specular(POWDER), snow_spec_types, fade_snow);
    var rock = vec3(0.0);
    var rock_spec = 0.0;
    if textured && cvg < 1.0 {
        let d1 = textureSampleGrad(base_texture, base_sampler, in.uv, uv_dx, uv_dy);
        rock = d1.rgb * d1.rgb;
        rock_spec = terrain.base[0].w * d1.a * d1.a;
    } else if cvg < 1.0 {
        let w = planar_weights(in.world_normal);
        var d1 = vec4(0.0);
        var d2 = vec4(0.0);
        if b != 1.0 { d1 = base_diffuse(0u, s, w); }
        if b != 0.0 { d2 = base_diffuse(1u, s, w); }
        rock = mix(d1.rgb * d1.rgb, d2.rgb * d2.rgb, b);
        rock_spec = mix(terrain.base[0].w * d1.a * d1.a, terrain.base[1].w * d2.a * d2.a, b);
    }
    let albedo = mix(rock, vec3(SNOW_ALBEDO), cvg);
    let specular = mix(rock_spec, snow_spec, cvg);

    let v = gl::to_eye(in.world_position.xyz);
    let receiver = (pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u;
    let shadow = gl::sun_shadow(wp, n, view_z, in.position.xy, receiver);
    let lit = gl::key_light(light, normal, v, ecc, shadow, gl::contact_shadow(in.world_position.xyz), 1.0);
    let diffuse_light = lit.rgb;
    let specular_light = lit.a;

    let ao = 1.0 + light.specular.w * (in.uv_b.x - 1.0);
    let ao_tint = mix(light.ramp_shade.rgb, vec3(1.0), ao);
    let r_view = (view_bindings::view.view_from_world * vec4(reflect(-v, normal), 0.0)).xyz;
    let cube = textureSampleGrad(reflect_cube, cube_sampler, r_view, dpdx(r_view), dpdy(r_view)).rgb;
    let f0 = terrain.reflect.y;
    let fresnel = f0 + (1.0 - f0) * (1.0 - abs(dot(normal, v)));
    let env = pow(max(fresnel * cube, vec3(1e-6)), vec3(terrain.reflect.x)) * (1.0 - cvg) * fade_snow;
    var glint = 0.0;
    if group != 2u {
        let tile = light.sparkle.x;
        let sp = textureSampleGrad(sparkle_normal, sparkle_sampler, s.p.xz * tile, s.dx.xz * tile, s.dy.xz * tile);
        glint = cvg * gl::snow_glint(light, sp.xyz * 2.0 - 1.0, normal, v, diffuse_light);
    }
    let col = diffuse_light * ao_tint * albedo
        + specular * (light.specular.rgb * specular_light * ao + env)
        + glint;

    let fogged = gl::apply_fog(light, col, in.world_position.w);
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(fogged, 1.0));
    return out;
}
