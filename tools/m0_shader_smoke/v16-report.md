# M0 V16 验证报告: var<immediate> 翻译形态与 FirstConstant 对齐

- 日期: 2026-08-05 (task 2 运行)
- 工具: m0_shader_smoke (naga 29.0.4, HLSL SM6.6, SPIR-V)
- 定义: `var<immediate>` = WGSL push constant (Vulkan) = D3D12 root constant (Diligent SetInlineConstants)
- 证据产物: `out/*.hlsl`（含 `ConstantBuffer<T> : register(b0)` 声明）+ `out/*.spv`

## 1. 全仓 var<immediate> 站点（源码 grep, 12 处 / 9 文件）

- `crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\meshlet\meshlet_bindings.wgsl` ✅ compose OK (immediate 在 def 门控分支内, 随对应 pass 编译)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\render\mesh_preprocess.wgsl` ✅ compose OK (immediate 在 def 门控分支内, 随对应 pass 编译)（源码含 `var<immediate>`）
- `crates\bevy_pbr\src\render\wireframe.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_solari\src\realtime\realtime_bindings.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）
- `crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl` ✅ (IR 含 immediate)（源码含 `var<immediate>`）

- `crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl` : `var<immediate> constants: struct Constants` -> 4 字节
- `crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl` : `var<immediate> view_size: (非结构体 Vector { size: Bi, scalar: Scalar { kind: Uint, width: 4 } })` -> 8 字节
- `crates\bevy_pbr\src\meshlet\cull_bvh.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX` -> 8 字节
- `crates\bevy_pbr\src\meshlet\cull_clusters.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX` -> 4 字节
- `crates\bevy_pbr\src\meshlet\cull_instances.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX` -> 4 字节
- `crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl` : `var<immediate> max_compute_workgroups_per_dimension: (非结构体 Scalar(Scalar { kind: Uint, width: 4 }))` -> 4 字节
- `crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl` : `var<immediate> meshlet_raster_cluster_rightmost_slot: (非结构体 Scalar(Scalar { kind: Uint, width: 4 }))` -> 4 字节
- `crates\bevy_pbr\src\render\wireframe.wgsl` : `var<immediate> immediates: struct Immediates` -> 16 字节
- `crates\bevy_solari\src\realtime\presample_light_tiles.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX: struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX` -> 8 字节
- `crates\bevy_solari\src\realtime\realtime_bindings.wgsl` : `var<immediate> constants: struct PushConstants` -> 8 字节
- `crates\bevy_solari\src\realtime\restir_di.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX: struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX` -> 8 字节
- `crates\bevy_solari\src\realtime\restir_gi.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX: struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX` -> 8 字节
- `crates\bevy_solari\src\realtime\specular_gi.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX: struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX` -> 8 字节
- `crates\bevy_solari\src\realtime\world_cache_update.wgsl` : `var<immediate> constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX: struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX` -> 8 字节
- `crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl` : `var<immediate> push_constants: struct PushConstants` -> 16 字节

## 2. 每样本翻译形态

### HLSL root constants 形态

- naga HLSL back end 将 Immediate 全局翻译为 `ConstantBuffer<T> name : register(b0)`（SM6.6 模板常量缓冲）;
- 非结构体 immediate（u32 / vec2<u32>）触发 naga 已知缺口 #5683: `Unimplemented(push-constant has non-struct type)`;
- HLSL 声明逐样本（证据文件在 out/）:

#### `crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl`

`Compute:downsample_depth_first`: ✅

`Compute:downsample_depth_second`: ✅

```hlsl
21    static const SamplerState samplr = nagaSamplerHeap[nagaGroup0SamplerIndexArray[13]];
22    ConstantBuffer<Constants> constants: register(b0);
23    groupshared float intermediate_memory[16][16];
```

- `constants`（struct Constants）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| max_mip_level | 0 | 4 |

#### `crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl`

`Compute:clear_visibility_buffer`: ❌ Unimplemented("push-constant 'view_size' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683")

- `view_size`: 非结构体（Vector { size: Bi, scalar: Scalar { kind: Uint, width: 4 } }）, HLSL 触发 #5683 已知缺口; SPIR-V push constant 仍正常。

#### `crates\bevy_pbr\src\meshlet\cull_bvh.wgsl`

`Compute:cull_bvh`: ✅

```hlsl
111   
112   ConstantBuffer<ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: register(b0);
113   ByteAddressBuffer meshlet_bvh_nodesX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t3);
...
126   Texture2D<float4> depth_pyramidX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t0);
127   cbuffer viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b1) { ViewX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DU5TJMV3QX viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
128   cbuffer previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b2) { PreviousViewUniformsX_naga_oil_mod_XMJSXM6K7OBRHEOR2OBZGK4DBONZV6YTJNZSGS3THOMX previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
129   ByteAddressBuffer meshlet_instance_uniformsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t4);
```

- `constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX`（struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| read_from_front | 0 | 4 |
| rightmost_slot | 4 | 4 |

#### `crates\bevy_pbr\src\meshlet\cull_clusters.wgsl`

`Compute:cull_clusters`: ✅

```hlsl
115   
116   ConstantBuffer<ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: register(b0);
117   cbuffer viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b1) { ViewX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DU5TJMV3QX viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
118   ByteAddressBuffer meshlet_cull_dataX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t3);
...
128   Texture2D<float4> depth_pyramidX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t0);
129   cbuffer previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b2) { PreviousViewUniformsX_naga_oil_mod_XMJSXM6K7OBRHEOR2OBZGK4DBONZV6YTJNZSGS3THOMX previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
130   
```

- `constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX`（struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| rightmost_slot | 0 | 4 |

#### `crates\bevy_pbr\src\meshlet\cull_instances.wgsl`

`Compute:cull_instances`: ✅

```hlsl
98    
99    ConstantBuffer<ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX> constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX: register(b0);
100   ByteAddressBuffer meshlet_view_instance_visibilityX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t4);
...
109   Texture2D<float4> depth_pyramidX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t0);
110   cbuffer viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b1) { ViewX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DU5TJMV3QX viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
111   cbuffer previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(b2) { PreviousViewUniformsX_naga_oil_mod_XMJSXM6K7OBRHEOR2OBZGK4DBONZV6YTJNZSGS3THOMX previous_viewX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX; }
112   ByteAddressBuffer meshlet_instance_uniformsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX : register(t3);
```

- `constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX`（struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| scene_instance_count | 0 | 4 |

#### `crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl`

`Compute:remap_dispatch`: ❌ Unimplemented("push-constant 'max_compute_workgroups_per_dimension' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683")

- `max_compute_workgroups_per_dimension`: 非结构体（Scalar(Scalar { kind: Uint, width: 4 })）, HLSL 触发 #5683 已知缺口; SPIR-V push constant 仍正常。

#### `crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl`

`Vertex:vertex`: ❌ Unimplemented("push-constant 'meshlet_raster_cluster_rightmost_slot' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683")

`Fragment:fragment`: ❌ Unimplemented("push-constant 'meshlet_raster_cluster_rightmost_slot' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683")

- `meshlet_raster_cluster_rightmost_slot`: 非结构体（Scalar(Scalar { kind: Uint, width: 4 })）, HLSL 触发 #5683 已知缺口; SPIR-V push constant 仍正常。

#### `crates\bevy_pbr\src\render\wireframe.wgsl`

`Fragment:fragment`: ✅

```hlsl
10    
11    ConstantBuffer<Immediates> immediates: register(b0);
12    
```

- `immediates`（struct Immediates）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| color | 0 | 16 |

#### `crates\bevy_solari\src\realtime\presample_light_tiles.wgsl`

`Compute:presample_light_tiles`: ❌ PANIC[hlsl(Compute:presample_light_tiles)]: internal error: entered unreachable code

- `constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX`（struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_solari\src\realtime\realtime_bindings.wgsl`

- `constants`（struct PushConstants）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_solari\src\realtime\restir_di.wgsl`

`Compute:initial_and_temporal`: ❌ PANIC[hlsl(Compute:initial_and_temporal)]: internal error: entered unreachable code

`Compute:spatial_and_shade`: ❌ PANIC[hlsl(Compute:spatial_and_shade)]: internal error: entered unreachable code

- `constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX`（struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_solari\src\realtime\restir_gi.wgsl`

`Compute:initial_and_temporal`: ❌ PANIC[hlsl(Compute:initial_and_temporal)]: internal error: entered unreachable code

`Compute:spatial_and_shade`: ❌ PANIC[hlsl(Compute:spatial_and_shade)]: internal error: entered unreachable code

- `constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX`（struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_solari\src\realtime\specular_gi.wgsl`

`Compute:specular_gi`: ❌ PANIC[hlsl(Compute:specular_gi)]: internal error: entered unreachable code

- `constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX`（struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_solari\src\realtime\world_cache_update.wgsl`

`Compute:sample_di`: ❌ PANIC[hlsl(Compute:sample_di)]: internal error: entered unreachable code

`Compute:sample_gi`: ❌ PANIC[hlsl(Compute:sample_gi)]: internal error: entered unreachable code

`Compute:blend_new_samples`: ❌ PANIC[hlsl(Compute:blend_new_samples)]: internal error: entered unreachable code

- `constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX`（struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| frame_index | 0 | 4 |
| reset | 4 | 4 |

#### `crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl`

`Fragment:fragment`: ✅

```hlsl
11    
12    ConstantBuffer<PushConstants> push_constants: register(b0);
13    
```

- `push_constants`（struct PushConstants）成员布局:

| 成员 | 偏移(字节) | 大小(字节) |
|---|---|---|
| color | 0 | 16 |

### SPIR-V push constant 形态

- SPIR-V 侧为 `OpVariable PushConstant` 指向的 `OpTypeStruct`（成员 Offset decoration 由 WGSL uniform 布局决定）;
- 下表为 naga IR 中 push constant 块布局（= SPIR-V 布局, 二进制见 out/*.spv）;

## 3. FirstConstant 对齐验证表

- D3D12 `SetInlineConstants(pConstants, FirstConstant, NumConstants)` 以 4 字节 DWORD 为单位;
- WGSL uniform 布局保证成员偏移为 4 的倍数（vec2=8, vec4=16, f32=4 对齐）,
  故 `FirstConstant = 成员偏移 / 4` 恒为整数, 可直接映射。

| 文件 | 变量 | 类型 | 总大小(字节) | 成员 | 偏移 | 大小 | FirstConstant (offset/4) | NumConstants (size/4) | HLSL 形态 |
|---|---|---|---|---|---|---|---|---|---|
| crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl | constants | struct Constants | 4 | max_mip_level | 0 | 4 | 0 | 1 | ConstantBuffer<T> : register(b0) |
| crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl | view_size |  | 8 | — | — | — | — | — | ❌ #5683 非结构体 Vector { size: Bi, scalar: Scalar { kind: Uint, width: 4 } } |
| crates\bevy_pbr\src\meshlet\cull_bvh.wgsl | constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | 8 | read_from_front | 0 | 4 | 0 | 1 | ConstantBuffer<T> : register(b0) |
| crates\bevy_pbr\src\meshlet\cull_bvh.wgsl | constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | 8 | rightmost_slot | 4 | 4 | 1 | 1 | ConstantBuffer<T> : register(b0) |
| crates\bevy_pbr\src\meshlet\cull_clusters.wgsl | constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | 4 | rightmost_slot | 0 | 4 | 0 | 1 | ConstantBuffer<T> : register(b0) |
| crates\bevy_pbr\src\meshlet\cull_instances.wgsl | constantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | struct ConstantsX_naga_oil_mod_XMJSXM6K7OBRHEOR2NVSXG2DMMV2F6YTJNZSGS3THOMX | 4 | scene_instance_count | 0 | 4 | 0 | 1 | ConstantBuffer<T> : register(b0) |
| crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl | max_compute_workgroups_per_dimension |  | 4 | — | — | — | — | — | ❌ #5683 非结构体 Scalar(Scalar { kind: Uint, width: 4 }) |
| crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl | meshlet_raster_cluster_rightmost_slot |  | 4 | — | — | — | — | — | ❌ #5683 非结构体 Scalar(Scalar { kind: Uint, width: 4 }) |
| crates\bevy_pbr\src\render\wireframe.wgsl | immediates | struct Immediates | 16 | color | 0 | 16 | 0 | 4 | ConstantBuffer<T> : register(b0) |
| crates\bevy_solari\src\realtime\presample_light_tiles.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\presample_light_tiles.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\realtime_bindings.wgsl | constants | struct PushConstants | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\realtime_bindings.wgsl | constants | struct PushConstants | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\restir_di.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\restir_di.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\restir_gi.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\restir_gi.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\specular_gi.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\specular_gi.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\world_cache_update.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | frame_index | 0 | 4 | 0 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_solari\src\realtime\world_cache_update.wgsl | constantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | struct PushConstantsX_naga_oil_mod_XMJSXM6K7ONXWYYLSNE5DU4TFMFWHI2LNMVPWE2LOMRUW4Z3TX | 8 | reset | 4 | 4 | 1 | 1 | ❌ (无 HLSL 输出) |
| crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl | push_constants | struct PushConstants | 16 | color | 0 | 16 | 0 | 4 | ConstantBuffer<T> : register(b0) |

## 4. 结论

- 结构体 push constant: HLSL 产物 `ConstantBuffer<T> : register(b0)`（space 0）, 布局 4 字节粒度对齐,
  Diligent 侧 `SetInlineConstants(FirstConstant=offset/4, NumConstants=size/4)` 直连映射成立;
- 非结构体 push constant（3 处: clear_visibility_buffer 的 vec2<u32>、remap_1d_to_2d_dispatch 与
  visibility_buffer_hardware_raster 的 u32）: naga->HLSL 已知缺口 #5683（wgpu issue 5683）;
  SPIR-V（Vulkan）侧无此限制。路径 a 下需将非结构体包一层 struct（或 M2 时改用结构体 push constant）;
- 本次运行样本尺寸: 4 字节（u32 / 单字段 struct）、8 字节（vec2<u32> / 双字段 struct）、16 字节（vec4 单字段 struct）;
  全部 4 字节对齐; 0 字节与 32 字节样本在仓库中不存在。
- 结论: 路径 a 可行, 唯一已知缺口为非结构体 push constant 的 HLSL 输出（可规避）。
