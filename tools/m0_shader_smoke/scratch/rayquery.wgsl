enable wgpu_ray_query;

@group(0) @binding(0) var tlas: acceleration_structure;

@compute @workgroup_size(1)
fn main() {
    var rq: ray_query;
    let ray = RayDesc(0xFFu, 0xFFu, 0.001, 100.0, vec3(0.0), vec3(0.0, 1.0, 0.0));
    rayQueryInitialize(&rq, tlas, ray);
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    _ = hit.t;
}
