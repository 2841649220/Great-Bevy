@group(0) @binding(0) var<storage, read_write> bufs: binding_array<Data>;

struct Data { v: u32 }

@compute @workgroup_size(1)
fn main() {
    _ = 1u;
}
