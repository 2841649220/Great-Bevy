//! The `util` family of the wgpu re-export surface (M1-4b-2: sourced from
//! wgpu-types where the types exist there; `BufferInitDescriptor` and
//! `make_spirv` are self-authored with the wgpu 29.0.4 shapes).

#[cfg(feature = "shader_format_spirv")]
use alloc::borrow::Cow;
use wgpu_types::BufferUsages;

/// Describes a [`Buffer`](super::Buffer) to be created with initial data
/// (wgpu's `util::BufferInitDescriptor` shape).
#[derive(Clone, Debug)]
pub struct BufferInitDescriptor<'a> {
    /// Debug label of the buffer.
    pub label: super::wgpu_compat::Label<'a>,
    /// Contents of the buffer.
    pub contents: &'a [u8],
    /// Usages of the buffer.
    pub usage: BufferUsages,
}

/// Converts SPIR-V bytes to a [`ShaderSource::SpirV`](super::ShaderSource::SpirV)
/// (wgpu's `util::make_spirv`).
#[cfg(feature = "shader_format_spirv")]
pub fn make_spirv(data: &[u8]) -> super::ShaderSource<'static> {
    let mut words = Vec::with_capacity(data.len() / 4);
    for chunk in data.chunks(4) {
        words.push(u32::from_le_bytes([
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
            chunk.get(3).copied().unwrap_or(0),
        ]));
    }
    super::ShaderSource::SpirV(Cow::Owned(words))
}

pub use wgpu_types::{
    DispatchIndirectArgs, DrawIndexedIndirectArgs, DrawIndirectArgs, TextureDataOrder,
};
