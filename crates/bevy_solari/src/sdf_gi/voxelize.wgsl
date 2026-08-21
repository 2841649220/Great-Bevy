// M6a (task 18.1): scene SDF voxelizer compute pass.
//
// Reads a compact scene representation (a list of signed-distance primitives
// in `primitives` - the CPU reference is `bevy_solari::sdf_gi::sdf::Primitive`)
// and writes one distance sample per cell of a uniform grid into
// `sdf_samples`, matching the `SceneSdf` layout (x-major, then y, then z).
//
// Dynamic objects re-voxelize a local region at lower frequency/resolution
// (the update-frequency degradation of construction §10); the primitive list
// is re-uploaded when it changes.
//
// The output buffer is the same shape the CPU reference `SceneSdf` produces,
// so the two can be compared directly for validation.

@group(0) @binding(0) var<storage, read> primitives: array<Primitive, 64u>;
@group(0) @binding(1) var<uniform> grid_info: GridInfo;
@group(0) @binding(2) var<storage, read_write> sdf_samples: array<f32>;

struct Primitive {
    kind: u32,
    a: vec4<f32>, // center / a
    b: vec4<f32>, // half_extents / b / plane normal
    radius: f32,
    distance: f32, // plane distance
};

struct GridInfo {
    origin: vec3<f32>,
    size: vec3<u32>,
    cell_size: f32,
    primitive_count: u32,
};

fn sd_sphere(p: vec3<f32>, center: vec3<f32>, radius: f32) -> f32 {
    return length(p - center) - radius;
}

fn sd_box(p: vec3<f32>, center: vec3<f32>, half_extents: vec3<f32>) -> f32 {
    let q = abs(p - center) - half_extents;
    return length(max(q, vec3(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

fn sd_plane(p: vec3<f32>, normal: vec3<f32>, distance: f32) -> f32 {
    return dot(p, normal) + distance;
}

fn sd_capsule(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>, radius: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h) - radius;
}

fn primitive_distance(p: vec3<f32>, prim: Primitive) -> f32 {
    if prim.kind == 0u {
        return sd_sphere(p, prim.a.xyz, prim.radius);
    } else if prim.kind == 1u {
        return sd_box(p, prim.a.xyz, prim.b.xyz);
    } else if prim.kind == 2u {
        return sd_plane(p, prim.a.xyz, prim.distance);
    } else {
        return sd_capsule(p, prim.a.xyz, prim.b.xyz, prim.radius);
    }
}

@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (any(gid >= grid_info.size)) {
        return;
    }
    let center = grid_info.origin
        + (vec3(f32(gid.x), f32(gid.y), f32(gid.z)) + vec3(0.5)) * grid_info.cell_size;

    var d = 1e30;
    for (var i = 0u; i < grid_info.primitive_count; i = i + 1u) {
        d = min(d, primitive_distance(center, primitives[i]));
    }

    let flat = (gid.z * grid_info.size.x * grid_info.size.y + gid.y * grid_info.size.x + gid.x);
    sdf_samples[flat] = d;
}