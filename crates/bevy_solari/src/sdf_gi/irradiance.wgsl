// M6a (task 18.3): single-bounce GI + last-frame irradiance cache.
//
// At probe resolution, samples the scene once (the AO output of the 18.2
// ray-march pass doubles as the single-bounce irradiance input for the
// dominant-light direction) and blends into the irradiance cache
// (temporal), then a spatial 3x3 filter. Mirrors the CPU reference
// `bevy_solari::sdf_gi::sdf::{IrradianceCache::blend,
// IrradianceCache::spatial_filter}`. Low resolution + temporal/spatial
// filtering keeps the cost well under frame budget.
//
// Bindings match `SdfGiPipelines::bind_group_layout` (init_sdf_gi_pipelines):
//   0  sdf_samples (storage, read)
//   1  grid_info   (uniform)
//   2  irradiance  (storage, read_write) - the cache
//   3  ao_output   (storage, read_write) - incoming single-bounce input
//   4  shadow_output(storage, read_write) - incoming shadow-weighted input
//   5  probe_info  (uniform, u32 x4: xy=probe dims, z=reset flag)

@group(0) @binding(0) var<storage, read> sdf_samples: array<f32>;
@group(0) @binding(1) var<uniform> grid_info: GridInfo;
@group(0) @binding(2) var<storage, read_write> irradiance: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> ao_input: array<f32>;
@group(0) @binding(4) var<storage, read_write> shadow_input: array<f32>;
@group(0) @binding(5) var<uniform> probe_info: vec4<u32>;

struct GridInfo {
    origin: vec3<f32>,
    size: vec3<u32>,
    cell_size: f32,
    primitive_count: u32,
};

fn index(x: u32, y: u32) -> u32 {
    return y * probe_info.x + x;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (any(gid.xy >= probe_info.xy)) {
        return;
    }
    let idx = index(gid.x, gid.y);

    // Single-bounce irradiance estimate for the dominant light: the AO
    // (openness) times a warm key-light color, attenuated by the soft
    // shadow term. A full per-pixel sky/hemisphere sampling replaces this
    // when the camera-facing GI composite lands.
    let ao = ao_input[idx];
    let shadow = shadow_input[idx];
    let key = vec3(1.0, 0.9, 0.7);
    let incoming = vec4(key * ao * shadow, 1.0);

    // Temporal blend (alpha=1.0 on reset, else 0.15 for stability).
    let alpha = select(0.15, 1.0, probe_info.z == 1u);
    let prev = irradiance[idx];
    irradiance[idx] = prev + (incoming - prev) * alpha;

    // Spatial 3x3 filter (clamped edges).
    var acc = vec3(0.0);
    var n = 0u;
    for (var dy = -1i32; dy <= 1i32; dy = dy + 1i32) {
        for (var dx = -1i32; dx <= 1i32; dx = dx + 1i32) {
            let nx = min(max(i32(gid.x) + dx, 0), i32(probe_info.x) - 1);
            let ny = min(max(i32(gid.y) + dy, 0), i32(probe_info.y) - 1);
            acc += irradiance[index(u32(nx), u32(ny))].xyz;
            n = n + 1u;
        }
    }
    irradiance[idx] = vec4(acc / f32(n), 1.0);
}
