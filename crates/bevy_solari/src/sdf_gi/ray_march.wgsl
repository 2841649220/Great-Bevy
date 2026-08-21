// M6a (task 18.2): per-probe software ray marching against the scene SDF.
// Produces SDF ambient occlusion + soft shadows, works on no-RT devices
// (Vulkan GLES/D3D12 compute-only). The math mirrors the CPU reference
// `bevy_solari::sdf_gi::sdf::{march, sdf_ao, soft_shadow}`.
//
// Bindings match `SdfGiPipelines::bind_group_layout` (init_sdf_gi_pipelines):
//   0  sdf_samples (storage, read)
//   1  grid_info   (uniform)
//   2  irradiance  (storage, read_write; written by the 18.3 pass)
//   3  ao_output   (storage, read_write)
//   4  shadow_output(storage, read_write)
//   5  probe_info  (uniform, u32 x4: xy=probe dims, z=reset)

@group(0) @binding(0) var<storage, read> sdf_samples: array<f32>;
@group(0) @binding(1) var<uniform> grid_info: GridInfo;
@group(0) @binding(2) var<storage, read_write> irradiance: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> ao_output: array<f32>;
@group(0) @binding(4) var<storage, read_write> shadow_output: array<f32>;
@group(0) @binding(5) var<uniform> probe_info: vec4<u32>;

struct GridInfo {
    origin: vec3<f32>,
    size: vec3<u32>,
    cell_size: f32,
    primitive_count: u32,
};

fn cell_index(x: u32, y: u32, z: u32) -> u32 {
    return z * grid_info.size.x * grid_info.size.y + y * grid_info.size.x + x;
}

// Trilinear-interpolated distance at a world point (matches `SceneSdf::distance_at`).
fn distance_at(p: vec3<f32>) -> f32 {
    let max = vec3(f32(grid_info.size.x - 1u), f32(grid_info.size.y - 1u), f32(grid_info.size.z - 1u));
    let g = clamp((p - grid_info.origin) / grid_info.cell_size - vec3(0.5), vec3(0.0), max);
    let i0 = floor(g);
    let i1 = ceil(g);
    let frac = g - i0;
    let x0 = u32(i0.x);
    let y0 = u32(i0.y);
    let z0 = u32(i0.z);
    let x1 = u32(i1.x);
    let y1 = u32(i1.y);
    let z1 = u32(i1.z);

    let c000 = sdf_samples[cell_index(x0, y0, z0)];
    let c100 = sdf_samples[cell_index(x1, y0, z0)];
    let c010 = sdf_samples[cell_index(x0, y1, z0)];
    let c110 = sdf_samples[cell_index(x1, y1, z0)];
    let c001 = sdf_samples[cell_index(x0, y0, z1)];
    let c101 = sdf_samples[cell_index(x1, y0, z1)];
    let c011 = sdf_samples[cell_index(x0, y1, z1)];
    let c111 = sdf_samples[cell_index(x1, y1, z1)];

    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        return a + (b - a) * t;
    }
    return lerp(
        lerp(lerp(c000, c100, frac.x), lerp(c010, c110, frac.x), frac.y),
        lerp(lerp(c001, c101, frac.x), lerp(c011, c111, frac.x), frac.y),
        frac.z,
    );
}

fn gradient_at(p: vec3<f32>) -> vec3<f32> {
    let h = 1e-2;
    return normalize(vec3(
        distance_at(p + vec3(h, 0.0, 0.0)) - distance_at(p - vec3(h, 0.0, 0.0)),
        distance_at(p + vec3(0.0, h, 0.0)) - distance_at(p - vec3(0.0, h, 0.0)),
        distance_at(p + vec3(0.0, 0.0, h)) - distance_at(p - vec3(0.0, 0.0, h)),
    ));
}

// Sphere-trace one ray; returns true on hit and stores the distance in
// `hit_distance` (module-scope, matches the CPU `march`).
var<private> hit_distance: f32;

fn march(ro: vec3<f32>, rd: vec3<f32>, max_distance: f32, steps: u32) -> bool {
    var t = 0.0;
    for (var i = 0u; i < steps; i = i + 1u) {
        let d = distance_at(ro + rd * t);
        if (d <= 1e-3) {
            hit_distance = t;
            return true;
        }
        if (t + d >= max_distance) {
            break;
        }
        t += d;
    }
    return false;
}

fn sdf_ao(p: vec3<f32>, normal: vec3<f32>) -> f32 {
    var ao = 0.0;
    for (var i = 1u; i <= 4u; i = i + 1u) {
        let dist = f32(i) / 4.0;
        let d = distance_at(p + normal * dist);
        ao += max(dist - d, 0.0);
    }
    return clamp(1.0 - ao / 4.0, 0.0, 1.0);
}

fn soft_shadow(ro: vec3<f32>, light_dir: vec3<f32>, k: f32) -> f32 {
    var res = 1.0;
    var t = 0.02;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let d = distance_at(ro + light_dir * t);
        if (d < 1e-4) {
            return 0.0;
        }
        res = min(res, k * d / t);
        if (t >= 10.0) {
            break;
        }
        t += d;
    }
    return clamp(res, 0.0, 1.0);
}

// One probe per thread. Probe (x, y) maps to the SDF grid: the ray origin
// sits at the probe cell center on a horizontal slice through the grid
// mid-height, looking toward the dominant light (a full camera-facing pass
// with per-pixel rays replaces this in 18.3).
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (any(gid.xy >= probe_info.xy)) {
        return;
    }
    let idx = gid.y * probe_info.x + gid.x;

    // Probe position in the SDF grid (horizontal slice at grid mid-height).
    let cell = vec3(
        f32(gid.x) + 0.5,
        f32(grid_info.size.y) * 0.5,
        f32(gid.y) + 0.5,
    );
    let ro = grid_info.origin + cell * grid_info.cell_size;

    // Dominant light direction (normalized; matches the CPU test shadow ray).
    let light_dir = normalize(vec3(0.5, 1.0, 0.3));
    var hit = march(ro, light_dir, 10.0, 64u);
    if (hit) {
        let p = ro + light_dir * hit_distance;
        let normal = gradient_at(p);
        ao_output[idx] = sdf_ao(p, normal);
        shadow_output[idx] = soft_shadow(p, light_dir, 8.0);
    } else {
        ao_output[idx] = 1.0;
        shadow_output[idx] = 1.0;
    }
}
