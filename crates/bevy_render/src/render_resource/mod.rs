mod atomic_pod;
mod batched_uniform_buffer;
mod bind_group;
mod bind_group_entries;
mod bind_group_layout;
mod bindless;
mod buffer;
mod buffer_vec;
mod gpu_array_buffer;
mod pipeline;
mod pipeline_cache;
mod pipeline_specializer;
mod sparse_buffer_vec;
mod specializer;
mod storage_buffer;
mod texture;
mod uniform_buffer;
pub(crate) mod util;
pub(crate) mod wgpu_compat;

pub use atomic_pod::*;
pub use bind_group::*;
pub use bind_group_entries::*;
pub use bind_group_layout::*;
pub use bindless::*;
pub use buffer::*;
pub use buffer_vec::*;
pub use gpu_array_buffer::*;
pub use pipeline::*;
pub use pipeline_cache::*;
pub use pipeline_specializer::*;
pub use sparse_buffer_vec::*;
pub use specializer::*;
pub use storage_buffer::*;
pub use texture::*;
pub use uniform_buffer::*;

// The re-export surface (M1-4b-2): the NAMES are unchanged; the sources are
// wgpu-types 29.0.4 (where the type exists there) or the shape-equivalent
// self-authored types in `wgpu_compat` (where the type was wgpu-runtime
// dependent). Consumer crates construct and field-access these types
// unchanged - see the M1-4b-2 report §2 for the per-name provenance.
pub use wgpu_types::{
    AccelerationStructureFlags, AccelerationStructureGeometryFlags,
    AccelerationStructureUpdateMode, AdapterInfo as WgpuAdapterInfo, AddressMode, AstcBlock,
    AstcChannel, BindGroupLayoutEntry, BindingType, BlasGeometrySizeDescriptors,
    BlasTriangleGeometrySizeDescriptor, BlendComponent, BlendFactor, BlendOperation, BlendState,
    BufferAddress, BufferBindingType, BufferSize, BufferUsages, ColorTargetState, ColorWrites,
    CompareFunction, DepthBiasState, DepthStencilState, DownlevelFlags, Extent3d, Face,
    Features as WgpuFeatures, FilterMode, FrontFace, ImageSubresourceRange, IndexFormat,
    Limits as WgpuLimits, LoadOp, MipmapFilterMode, MultisampleState, Operations, Origin3d,
    PolygonMode, PrimitiveState, PrimitiveTopology, SamplerBindingType, ShaderStages,
    StencilFaceState, StencilOperation, StencilState, StorageTextureAccess, StoreOp,
    TexelCopyBufferLayout, TextureAspect, TextureDimension, TextureFormat,
    TextureFormatFeatureFlags, TextureFormatFeatures, TextureSampleType, TextureUsages,
    TextureViewDimension, VertexAttribute, VertexFormat, VertexStepMode, COPY_BUFFER_ALIGNMENT,
};

pub use wgpu_compat::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, Blas, BlasBuildEntry, BlasGeometries,
    BlasTriangleGeometry, BufferAsyncError, BufferBinding, BufferDescriptor, CommandEncoder,
    CommandEncoderDescriptor, ComputePass, ComputePassDescriptor,
    ComputePipelineDescriptor as RawComputePipelineDescriptor, CreateBlasDescriptor,
    CreateTlasDescriptor, FragmentState as RawFragmentState, MapMode, PipelineCompilationOptions,
    PipelineLayout, PipelineLayoutDescriptor, PollType, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor as RawRenderPipelineDescriptor, SamplerDescriptor, ShaderModule,
    ShaderModuleDescriptor, ShaderSource, TexelCopyBufferInfo, TexelCopyTextureInfo,
    TextureDescriptor, TextureViewDescriptor, Tlas, TlasInstance,
    VertexBufferLayout as RawVertexBufferLayout, VertexState as RawVertexState, WgpuSampler,
    WgpuTextureView,
};

pub use util::{
    BufferInitDescriptor, DispatchIndirectArgs, DrawIndexedIndirectArgs, DrawIndirectArgs,
    TextureDataOrder,
};

pub mod encase {
    pub use bevy_encase_derive::ShaderType;
    pub use encase::*;
}

pub use self::encase::{ShaderSize, ShaderType};

pub use naga::ShaderStage;

pub use bevy_material::{
    bind_group_layout_entries::{
        binding_types, BindGroupLayoutEntries, BindGroupLayoutEntryBuilder,
        DynamicBindGroupLayoutEntries, IntoBindGroupLayoutEntryBuilder,
        IntoBindGroupLayoutEntryBuilderArray, IntoIndexedBindGroupLayoutEntryBuilderArray,
    },
    descriptor::{
        BindGroupLayoutDescriptor, CachedComputePipelineId, CachedRenderPipelineId,
        ComputePipelineDescriptor, FragmentState, PipelineDescriptor, RenderPipelineDescriptor,
        VertexState,
    },
    specialize::SpecializedMeshPipelineError,
};
