@group(0) @binding(0) var<storage, read_write> bufs: binding_array<Data>;

struct Data { v: u32 }

struct Push { idx: u32 }
var<immediate> pc: Push;

@compute @workgroup_size(1)
fn main() {
    bufs[pc.idx].v = 7u;
}
