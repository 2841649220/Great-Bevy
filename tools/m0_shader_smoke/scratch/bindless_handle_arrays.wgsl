// Probe: top-level consumer that imports and REFERENCES all 9 handle-space
// unbounded binding arrays from bevy_render::bindless, so naga_oil cannot
// cull any of them. Compiled through the real composer (registration of
// bindless.wgsl + MATERIAL_BIND_GROUP=3 interpolation) via:
//   m0_shader_smoke <root> --consumer bindless_handle_arrays.wgsl --defs bindless_handle_arrays=BINDLESS

#import bevy_render::bindless::{
    bindless_samplers_filtering,
    bindless_samplers_non_filtering,
    bindless_samplers_comparison,
    bindless_textures_1d,
    bindless_textures_2d,
    bindless_textures_2d_array,
    bindless_textures_3d,
    bindless_textures_cube,
    bindless_textures_cube_array,
}

@group(0) @binding(0) var<uniform> idx: u32;
@group(0) @binding(1) var<storage, read_write> sink: f32;

@compute @workgroup_size(1)
fn main() {
    let i = idx;
    let s = bindless_samplers_filtering[i];
    let s2 = bindless_samplers_non_filtering[i];
    let t1 = textureLoad(bindless_textures_1d[i], 0, 0);
    let t2 = textureLoad(bindless_textures_2d[i], vec2<i32>(0), 0);
    let t2a = textureLoad(bindless_textures_2d_array[i], vec2<i32>(0), 0, 0);
    let t3 = textureLoad(bindless_textures_3d[i], vec3<i32>(0), 0);
    let tc = textureSampleLevel(bindless_textures_cube[i], s, vec3(0.5), 0.0);
    let tca = textureSampleLevel(bindless_textures_cube_array[i], s, vec3(0.5), 0, 0.0);
    let v = textureSampleLevel(bindless_textures_2d[i], s, vec2(0.5), 0.0);
    let v2 = textureSampleLevel(bindless_textures_2d[i], s2, vec2(0.5), 0.0);
    let cmp = textureSampleLevel(
        bindless_textures_2d[i],
        bindless_samplers_comparison[i],
        vec2(0.5),
        0.0,
    );
    let sum = t1.x + t2.x + t2a.x + t3.x + tc.x + tca.x + v.x + v2.x;
    sink = sum + cmp.x;
}
