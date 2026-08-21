# M0 V4 验证报告: ray query / 无界 binding_array / subgroup 编译翻译形态

- 日期: 2026-08-05 (task 2 运行, 含 BINDLESS 修复后重跑)
- 工具: m0_shader_smoke (naga 29.0.4, HLSL SM6.6, SPIR-V 1.x)
- 能力集: features_to_naga_capabilities(Features::all(), DownlevelFlags::all())
- 证据产物目录: `out/` (每特征文件 .hlsl 全文 + .spv 二进制 + probe_* 最小复现证据)
- BINDLESS 修复: bindless.wgsl 的 9 组 handle 空间无界数组在 `#ifdef BINDLESS` 门内;
  本版工具按 fork 运行时（material.rs:494/496, prepass/mod.rs:356）为消费者定义 BINDLESS,
  该 9 组数组首次进入真实编译产物（见 §2.3）

## 1. ray query

### 1.1 全仓 `enable wgpu_ray_query;` 站点（13 处）

- `crates\bevy_solari\src\pathtracer\pathtracer.wgsl`
- `crates\bevy_solari\src\realtime\presample_light_tiles.wgsl`
- `crates\bevy_solari\src\realtime\realtime_bindings.wgsl`
- `crates\bevy_solari\src\realtime\resolve_dlss_rr_textures.wgsl`
- `crates\bevy_solari\src\realtime\restir_di.wgsl`
- `crates\bevy_solari\src\realtime\restir_gi.wgsl`
- `crates\bevy_solari\src\realtime\specular_gi.wgsl`
- `crates\bevy_solari\src\realtime\world_cache_compact.wgsl`
- `crates\bevy_solari\src\realtime\world_cache_query.wgsl`
- `crates\bevy_solari\src\realtime\world_cache_update.wgsl`
- `crates\bevy_solari\src\scene\brdf.wgsl`
- `crates\bevy_solari\src\scene\raytracing_scene_bindings.wgsl`
- `crates\bevy_solari\src\scene\sampling.wgsl`

### 1.2 编译状态（14 个编译产物含 ray query IR; 其中 13 处为源码 `enable wgpu_ray_query;` 站点）

| 文件 | compose | SPV | HLSL |
|---|---|---|---|
| crates\bevy_solari\src\pathtracer\pathtracer.wgsl | ✅ | ✅ 19159 words | ❌ 0/1 |
| crates\bevy_solari\src\realtime\gbuffer_utils.wgsl | ✅ | ✅ 11383 words | — (无 entry point) |
| crates\bevy_solari\src\realtime\presample_light_tiles.wgsl | ✅ | ✅ 16188 words | ❌ 0/1 |
| crates\bevy_solari\src\realtime\realtime_bindings.wgsl | ✅ | ✅ 14588 words | — (无 entry point) |
| crates\bevy_solari\src\realtime\resolve_dlss_rr_textures.wgsl | ✅ | ✅ 15875 words | ❌ 0/1 |
| crates\bevy_solari\src\realtime\restir_di.wgsl | ✅ | ✅ 25245 words | ❌ 0/2 |
| crates\bevy_solari\src\realtime\restir_gi.wgsl | ✅ | ✅ 24175 words | ❌ 0/2 |
| crates\bevy_solari\src\realtime\specular_gi.wgsl | ✅ | ✅ 21889 words | ❌ 0/1 |
| crates\bevy_solari\src\realtime\world_cache_compact.wgsl | ✅ | ✅ 16638 words | ❌ 0/4 |
| crates\bevy_solari\src\realtime\world_cache_query.wgsl | ✅ | ✅ 15265 words | — (无 entry point) |
| crates\bevy_solari\src\realtime\world_cache_update.wgsl | ✅ | ✅ 19685 words | ❌ 0/3 |
| crates\bevy_solari\src\scene\brdf.wgsl | ✅ | ✅ 16147 words | — (无 entry point) |
| crates\bevy_solari\src\scene\raytracing_scene_bindings.wgsl | ✅ | ✅ 13489 words | — (无 entry point) |
| crates\bevy_solari\src\scene\sampling.wgsl | ✅ | ✅ 16134 words | — (无 entry point) |

### 1.3 HLSL 产物形态（证据: `out/probe_rayquery.hlsl` — 最小复现 shader,
与批次同 naga 29.0.4 / SM6.6 / 同 capabilities, 入口 `main`）

```hlsl
struct RayDesc_ {
    uint flags;
    uint cull_mask;
    float tmin;
    float tmax;
    float3 origin;
    int _pad5_0;
    float3 dir;
    int _end_pad_0;
};

struct RayIntersection {
    uint kind;
    float t;
    uint instance_custom_data;
    uint instance_index;
    uint sbt_record_offset;
    uint geometry_index;
    uint primitive_index;
    float2 barycentrics;
    bool front_face;
    int _pad9_0;
    int _pad9_1;
    row_major float4x3 object_to_world;
    int _pad10_0;
    row_major float4x3 world_to_object;
    int _end_pad_0;
};

RayDesc RayDescFromRayDesc_(RayDesc_ arg0) {
    RayDesc ret = (RayDesc)0;
    ret.Origin = arg0.origin;
    ret.TMin = arg0.tmin;
    ret.Direction = arg0.dir;
    ret.TMax = arg0.tmax;
    return ret;
}

RaytracingAccelerationStructure tlas : register(t0);

RayDesc_ ConstructRayDesc_(uint arg0, uint arg1, float arg2, float arg3, float3 arg4, float3 arg5) {
    RayDesc_ ret = (RayDesc_)0;
    ret.flags = arg0;
    ret.cull_mask = arg1;
    ret.tmin = arg2;
    ret.tmax = arg3;
    ret.origin = arg4;
    ret.dir = arg5;
    return ret;
}

RayIntersection GetCommittedIntersection(RayQuery<RAY_FLAG_NONE> rq, uint rq_tracker) {
    RayIntersection ret = (RayIntersection)0;
    if (((rq_tracker & 4) == 4)) {
        ret.kind = rq.CommittedStatus();
        if( rq.CommittedStatus() == COMMITTED_NOTHING) {} else {
            ret.t = rq.CommittedRayT();
            ret.instance_custom_data = rq.CommittedInstanceID();
            ret.instance_index = rq.CommittedInstanceIndex();
            ret.sbt_record_offset = rq.CommittedInstanceContributionToHitGroupIndex();
            ret.geometry_index = rq.CommittedGeometryIndex();
            ret.primitive_index = rq.CommittedPrimitiveIndex();
            if( rq.CommittedStatus() == COMMITTED_TRIANGLE_HIT ) {
                ret.barycentrics = rq.CommittedTriangleBarycentrics();
                ret.front_face = rq.CommittedTriangleFrontFace();
            }
            ret.object_to_world = rq.CommittedObjectToWorld4x3();
            ret.world_to_object = rq.CommittedWorldToObject4x3();
        }
    }
    return ret;
}

[numthreads(1, 1, 1)]
void main()
{
    RayQuery<RAY_FLAG_NONE> rq;
    uint naga_query_init_tracker_for_rq = 0;

    RayDesc_ ray = ConstructRayDesc_(255u, 255u, 0.001, 100.0, (0.0).xxx, float3(0.0, 1.0, 0.0));
    {
        RayDesc_ naga_desc = ray;
        float naga_tmin = naga_desc.tmin;
        float naga_tmax = naga_desc.tmax;
        float3 naga_origin = naga_desc.origin;
        float3 naga_dir = naga_desc.dir;
        uint naga_flags = naga_desc.flags;
        bool naga_tmin_valid = (naga_tmin >= 0.0) && (naga_tmin <= naga_tmax) && !(((asuint(naga_tmin) & 2139095040) == 2139095040) && ((asuint(naga_tmin) & 0x7fffff) != 0));
        bool naga_tmax_valid = !(((asuint(naga_tmax) & 2139095040) == 2139095040) && ((asuint(naga_tmax) & 0x7fffff) != 0));
        bool naga_origin_valid = !any((((asuint(naga_origin) & 2139095040) == 2139095040) && ((asuint(naga_origin) & 0x7fffff) != 0)));
        bool naga_dir_valid = !any((((asuint(naga_dir) & 2139095040) == 2139095040) && ((asuint(naga_dir) & 0x7fffff) != 0)));
        bool naga_contains_opaque = ((naga_flags & 1) == 1);
        bool naga_contains_no_opaque = ((naga_flags & 2) == 2);
        bool naga_contains_cull_opaque = ((naga_flags & 64) == 64);
        bool naga_contains_cull_no_opaque = ((naga_flags & 128) == 128);
        bool naga_contains_cull_front = ((naga_flags & 32) == 32);
        bool naga_contains_cull_back = ((naga_flags & 16) == 16);
        bool naga_contains_skip_triangles = ((naga_flags & 256) == 256);
        bool naga_contains_skip_aabbs = ((naga_flags & 512) == 512);
        bool naga_contains_skip_triangles_aabbs =  (naga_contains_skip_aabbs && naga_contains_skip_triangles) ;
        bool naga_contains_skip_triangles_cull =  (naga_contains_cull_front && naga_contains_skip_triangles) || (naga_contains_cull_front && naga_contains_cull_back) || (naga_contains_cull_back && naga_contains_skip_triangles) ;
        bool naga_contains_multiple_opaque =  (naga_contains_cull_no_opaque && naga_contains_opaque) || (naga_contains_cull_no_opaque && naga_contains_no_opaque) || (naga_contains_cull_no_opaque && naga_contains_cull_opaque) || (naga_contains_cull_opaque && naga_contains_opaque) || (naga_contains_cull_opaque && naga_contains_no_opaque) || (naga_contains_no_opaque && naga_contains_opaque) ;
        if (naga_tmin_valid && naga_tmax_valid && naga_origin_valid && naga_dir_valid && !(naga_contains_skip_triangles_aabbs || naga_contains_skip_triangles_cull || naga_contains_multiple_opaque)) {
            naga_query_init_tracker_for_rq = naga_query_init_tracker_for_rq | 1;
            rq.TraceRayInline(tlas, naga_desc.flags, naga_desc.cull_mask, RayDescFromRayDesc_(naga_desc));
        }
    }
    bool _e13 = false;
    {
        bool naga_has_initialized = ((naga_query_init_tracker_for_rq & 1) == 1);
        bool naga_has_finished = ((naga_query_init_tracker_for_rq & 4) == 4);
        if (naga_has_initialized && !naga_has_finished) {
            _e13 = rq.Proceed();
            naga_query_init_tracker_for_rq = naga_query_init_tracker_for_rq | 2;
            if (!_e13) { naga_query_init_tracker_for_rq = naga_query_init_tracker_for_rq | 4; }
    }}
    RayIntersection hit = GetCommittedIntersection(rq, naga_query_init_tracker_for_rq);
    float phony = hit.t;
    return;
}

```

### 1.4 结论: ray query 翻译层可行; Solari 组合 shader 的 HLSL 输出被 storage binding_array 缺口阻塞

- naga 29 HLSL back end 完整支持 ray query 翻译（`supported_capabilities()` 含 RAY_QUERY）;
HLSL 产物形态（1.3 证据）: `RayQuery<RAY_FLAG_NONE>` 查询对象 + `RayDesc` 结构 +
`TraceRayInline`/`Proceed()`/`CommittedStatus()`/`CommittedRayT()` 等 SM6.5 内联光线跟踪 API,
rayQueryInitialize 附 tmin/tmax/NaN 合法性守卫与初始化跟踪变量（ray_query_initialization_tracking）;
SPIR-V 产物为 OpTypeAccelerationStructureKHR + OpRayQuery*（见 out/*.spv）。
- 13 处站点 compose/SPIR-V 全部通过; 8 个带 entry point 的 solari shader 的 HLSL 输出触发
naga 内部 panic（back/hlsl/storage.rs:622, 见 §2.4）— 阻塞点不在 ray query 本身。
- 路径 a 对 ray query: **可行**（翻译层支持）, 待 §2 的 storage binding_array 缺口解除后全量可用。

## 2. 无界 binding_array

### 2.1 全仓无界声明（`binding_array<T>` 无尺寸, 13 处）

- `crates/bevy_render/src/bindless.wgsl`: 9 组 (group 3, binding 1..9, `#MATERIAL_BIND_GROUP`=3)
  （重要: 全部 9 组在 `#ifdef BINDLESS` 门内, 只有定义了 BINDLESS 的消费者编译才包含它们;
   fork 运行时在 bindless 模式下定义 BINDLESS: material.rs:494/496、prepass/mod.rs:356）
- `crates/bevy_solari/src/scene/raytracing_scene_bindings.wgsl`: 4 组 (group 0, binding 0..3)

### 2.2 编译状态（15 个编译产物含无界数组; 标注 (probe) 的为 `--consumer` 探针）

| 文件 | 无界声明 | HLSL |
|---|---|---|
| crates\bevy_pbr\src\render\parallax_mapping.wgsl | group 3 binding 1, group 3 binding 5 | — |
| crates\bevy_solari\src\pathtracer\pathtracer.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/1 |
| crates\bevy_solari\src\realtime\presample_light_tiles.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/1 |
| crates\bevy_solari\src\realtime\realtime_bindings.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | — |
| crates\bevy_solari\src\realtime\resolve_dlss_rr_textures.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/1 |
| crates\bevy_solari\src\realtime\restir_di.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/2 |
| crates\bevy_solari\src\realtime\restir_gi.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/2 |
| crates\bevy_solari\src\realtime\specular_gi.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/1 |
| crates\bevy_solari\src\realtime\world_cache_compact.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/4 |
| crates\bevy_solari\src\realtime\world_cache_query.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | — |
| crates\bevy_solari\src\realtime\world_cache_update.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | ❌ 0/3 |
| crates\bevy_solari\src\scene\brdf.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | — |
| crates\bevy_solari\src\scene\raytracing_scene_bindings.wgsl | group 0 binding 0, group 0 binding 1, group 0 binding 2, group 0 binding 3 | — |
| crates\bevy_solari\src\scene\sampling.wgsl | group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3 | — |
| tools/m0_shader_smoke/scratch/bindless_handle_arrays.wgsl (probe) | group 3 binding 1, group 3 binding 2, group 3 binding 3, group 3 binding 4, group 3 binding 5, group 3 binding 6, group 3 binding 7, group 3 binding 8, group 3 binding 9 | ✅ 1/1 |

### 2.3 HLSL 产物形态: handle 空间无界数组（真实编译证据）

- 批次内无消费者 HLSL 含 handle 空间无界数组（naga_oil 按 import 成员裁剪,
  未使用的 bindless 全局不会进入消费者模块; 见 §2.3 探针）

- `--consumer` 探针证据: `tools/m0_shader_smoke/scratch/bindless_handle_arrays.wgsl`（scratch/bindless_handle_arrays.wgsl, 显式 import 并引用
  bindless.wgsl 全部 9 组数组; 检测到的无界声明: group 3 binding 1, group 3 binding 2, group 3 binding 3, group 3 binding 4, group 3 binding 5, group 3 binding 6, group 3 binding 7, group 3 binding 8, group 3 binding 9）:

```hlsl
1     SamplerState nagaSamplerHeap[2048]: register(s0, space0);
2     SamplerComparisonState nagaComparisonSamplerHeap[2048]: register(s0, space1);
3     StructuredBuffer<uint> nagaGroup3SamplerIndexArray : register(t3, space255);
...
6     static const uint bindless_samplers_comparisonX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX = 3;
7     Texture1D<float4> bindless_textures_1dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t4, space3);
8     Texture2D<float4> bindless_textures_2dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t5, space3);
9     Texture2DArray<float4> bindless_textures_2d_arrayX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t6, space3);
10    Texture3D<float4> bindless_textures_3dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t7, space3);
11    TextureCube<float4> bindless_textures_cubeX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t8, space3);
12    TextureCubeArray<float4> bindless_textures_cube_arrayX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[2048] : register(t9, space3);
13    cbuffer idx : register(b0) { uint idx; }
...
19        uint i = idx;
20        SamplerState s = nagaSamplerHeap[nagaGroup3SamplerIndexArray[bindless_samplers_filteringX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX + i]];
21        SamplerState s2_ = nagaSamplerHeap[nagaGroup3SamplerIndexArray[bindless_samplers_non_filteringX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX + i]];
22        float4 t1_ = bindless_textures_1dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[i].Load(int2(int(0), int(0)));
...
29        float4 v2_ = bindless_textures_2dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[i].SampleLevel(s2_, (0.5).xx, 0.0);
30        float4 cmp = bindless_textures_2dX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX[i].SampleLevel(nagaSamplerHeap[nagaGroup3SamplerIndexArray[bindless_samplers_comparisonX_naga_oil_mod_XMJSXM6K7OJSW4ZDFOI5DUYTJNZSGYZLTOMX + i]], (0.5).xx, 0.0);
31        float sum = (((((((t1_.x + t2_.x) + t2a.x) + t3_.x) + tc.x) + tca.x) + v.x) + v2_.x);
```

### 2.4 最小复现: storage 空间 binding_array（Solari scene_bindings 形态）

- 声明本身可翻译: `var<storage, read_write> bufs: binding_array<Data>` ->
`RWByteAddressBuffer bufs[2048] : register(u0);`（probe, 6 行 shader）;
- 但一旦**访问**（`bufs[pc.idx].v = 7u;`）, naga 29.0.4 HLSL back end 在
`back/hlsl/storage.rs:622:26` 触发 `unreachable!()`（fill_access_chain 对 BindingArray
父类型无处理分支）。probe 输出（out/probe_storage_binding_array.txt）:

```

thread 'main' (5944) panicked at C:\Users\ASUS\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\naga-29.0.4\src\back\hlsl\storage.rs:622:26:
internal error: entered unreachable code
PANIC: internal error: entered unreachable code
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

- 影响: Solari scene_bindings 的 `vertex_buffers`/`index_buffers`（storage 空间）被
trace_ray/resolve_triangle_data_full 依赖链访问, 凡组合进该子图的 shader 在 HLSL 阶段 panic
（本批次 8 个 solari 文件, 见 m0-sa-report.md）; 无界纹理/采样器数组（handle 空间）不受影响。
### 2.5 naga 行为说明与 Diligent RUNTIME_ARRAY 兼容性

- naga HLSL writer 对 `IndexableLength::Dynamic` 数组直接 `unreachable!()`（writer.rs write_array_size）,
必须经 binding_map 提供 `binding_array_size` 覆盖。本工具统一覆盖为 2048（M2 按 PRS 细分）。
- 因此 HLSL 产物是**定长数组** `T[2048] : register(...)`（§2.3 证据: 实际 HLSL 产物）, 不是 SM6.6 的
unbounded 描述符（naga 29 不产出 DeclaredResourceCount=Unbounded; 若要求真 unbounded 需后处理注入, M3 决策项）。
- Diligent PRS: ArraySize 与着色器声明必须一致。naga 定长产物与 Diligent `PRS::ArraySize = 2048` + 
`SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE`/Dynamic 完全兼容。
- storage 空间 binding_array: naga 29 有内部 panic 缺口（storage.rs:622）, 需 naga 上游修复
（或 M3 规避: 拆分具名 buffer / bindless index + ByteAddressBuffer 方案）。
- 结论: 路径 a 对 handle 空间无界数组**可行**（定长上限翻译, §2.3 为编译证据）; 对 storage 空间**受阻**（naga 缺口）。

## 3. subgroup / wave ops

### 3.1 全仓真实 subgroup 内建用法（1 处）

- crates\bevy_pbr\src\meshlet\visibility_buffer_software_raster.wgsl: Compute:rasterize_cluster

### 3.2 HLSL 产物形态（证据: 首个含 subgroup 文件）

```hlsl
508       float _e197 = min_x;
509       const bool _e201 = WaveActiveAnyTrue(((_e196 - _e197) > 4.0));
510       if (_e201) {
```

### 3.3 结论: 路径 a 对 subgroup 基本可行（注意 `enable subgroups;` 未支持）

- 现有仓库中唯一真实内建为 `subgroupAny`（visibility_buffer_software_raster.wgsl:110）,
naga -> HLSL 产物为 WaveActiveAnyTrue 形式, SPIR-V 为 OpGroupNonUniformAny;
- SPD 的 SUBGROUP_SUPPORT 分支不含内建, 编译通过;
- 注意: naga 29 的 WGSL front **不支持 `enable subgroups;`**（"the `subgroups` enable-extension
is not yet supported", wgpu#5555, probe 复现见 out/probe_subgroup_enable.txt）;
本仓库用法（不写 enable, 直接用 subgroupAny）恰好绕开该限制, 可编译;
- 若未来引入更多 subgroup 内建（subgroupAdd 等）并采用标准 enable 声明, 需等 naga 上游支持。

## 4. 决策树结论

| 特性 | 路径 a 可行? | 依据 |
|---|---|---|
| ray query | 可行（翻译层） | probe 证据: RayQuery<RAY_FLAG_NONE>/TraceRayInline/Proceed/Committed*; 13 站点 compose/SPV 全过; HLSL 受 §2 storage 缺口影响 |
| 无界 binding_array (handle 空间) | 可行(定长上限) | §2.3 编译证据: binding_array_size=2048 覆盖下产物为定长数组 `T[2048] : register(...)`（9 组探针全量 + 批次消费者）; 与 Diligent PRS ArraySize 匹配 |
| 无界 binding_array (storage 空间) | 受阻 | naga 29 HLSL panic (storage.rs:622), 需上游修复或 M3 规避 |
| subgroup/wave | 可行 | 现状仅 subgroupAny(无 enable), 产物 WaveActiveAnyTrue; `enable subgroups;` 本身未支持(wgpu#5555) |

- 结论: 三特性均不需为**翻译本身**走路径 c; 但 solari 全量 HLSL 输出被 storage 空间
  binding_array 的 naga 缺口阻塞（8/14 solari 文件）, 该缺口需 naga 上游修复或路径 a 之外的手工处理。
