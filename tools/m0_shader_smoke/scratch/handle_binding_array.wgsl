@group(0) @binding(0) var texs: binding_array<texture_2d<f32>>;
@group(0) @binding(1) var samps: binding_array<sampler>;

struct Push { idx: u32 }
var<immediate> pc: Push;

@compute @workgroup_size(1)
fn main() {
    _ = textureSampleLevel(texs[pc.idx], samps[pc.idx], vec2(0.5), 0.0);
}
