# M0 S-A 批量编译冒烟报告（task 2 更新）

- 日期: 2026-08-05 (task 2 运行)
- 修复内容: import 注册改为依赖序（模仿 bevy_shader::ShaderCache::add_import_to_composer）
- naga 29.0.4 / naga_oil 0.22.0 (Cargo.lock 锁定版本)
- capabilities: wgpu-naga-bridge features_to_naga_capabilities(Features::all(), DownlevelFlags::all())
- HLSL: SM6.6, 每 entry point 一次编译 (wgpu-hal 调用模式); 无界数组 binding_array_size 覆盖 = 2048
- shader defs 镜像 fork 运行时: 新增 BINDLESS（material.rs:494/496 vertex+fragment、prepass/mod.rs:356）
  → pbr.wgsl/pbr_fragment.wgsl/pbr_prepass.wgsl/pbr_prepass_functions.wgsl/parallax_mapping.wgsl
  → bindless.wgsl 的 9 组 handle 空间无界数组（#ifdef BINDLESS 门内）随消费者进入编译
- 统计: 共 156 文件; compose+validate OK 156; SPIR-V OK 156; HLSL entry OK 105/124; 失败文件 11
- 验收拆分: 非 solari 142 文件失败 3; solari 14 文件失败 8

## 失败清单（真失败, import 修复后仍失败）

| 文件 | 分类 | 失败原因 |
|---|---|---|
| crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl | known-gap(naga) | hlsl(Compute:clear_visibility_buffer): Unimplemented("push-constant 'view_size' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683") |
| crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl | known-gap(naga) | hlsl(Compute:remap_dispatch): Unimplemented("push-constant 'max_compute_workgroups_per_dimension' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683") |
| crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl | known-gap(naga) | hlsl(Vertex:vertex): Unimplemented("push-constant 'meshlet_raster_cluster_rightmost_slot' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683")<br>hlsl(Fragment:fragment): Unimplemented("push-constant 'meshlet_raster_cluster_rightmost_slot' has non-struct type; tracked by: https://github.com/gfx-rs/wgpu/issues/5683") |
| crates\bevy_solari\src\pathtracer\pathtracer.wgsl | known-gap(naga internal panic) | hlsl(Compute:pathtrace): PANIC[hlsl(Compute:pathtrace)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\presample_light_tiles.wgsl | known-gap(naga internal panic) | hlsl(Compute:presample_light_tiles): PANIC[hlsl(Compute:presample_light_tiles)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\resolve_dlss_rr_textures.wgsl | known-gap(naga internal panic) | hlsl(Compute:resolve_dlss_rr_textures): PANIC[hlsl(Compute:resolve_dlss_rr_textures)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\restir_di.wgsl | known-gap(naga internal panic) | hlsl(Compute:initial_and_temporal): PANIC[hlsl(Compute:initial_and_temporal)]: internal error: entered unreachable code<br>hlsl(Compute:spatial_and_shade): PANIC[hlsl(Compute:spatial_and_shade)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\restir_gi.wgsl | known-gap(naga internal panic) | hlsl(Compute:initial_and_temporal): PANIC[hlsl(Compute:initial_and_temporal)]: internal error: entered unreachable code<br>hlsl(Compute:spatial_and_shade): PANIC[hlsl(Compute:spatial_and_shade)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\specular_gi.wgsl | known-gap(naga internal panic) | hlsl(Compute:specular_gi): PANIC[hlsl(Compute:specular_gi)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\world_cache_compact.wgsl | known-gap(naga internal panic) | hlsl(Compute:decay_world_cache): PANIC[hlsl(Compute:decay_world_cache)]: internal error: entered unreachable code<br>hlsl(Compute:compact_world_cache_single_block): PANIC[hlsl(Compute:compact_world_cache_single_block)]: internal error: entered unreachable code<br>hlsl(Compute:compact_world_cache_blocks): PANIC[hlsl(Compute:compact_world_cache_blocks)]: internal error: entered unreachable code<br>hlsl(Compute:compact_world_cache_write_active_cells): PANIC[hlsl(Compute:compact_world_cache_write_active_cells)]: internal error: entered unreachable code |
| crates\bevy_solari\src\realtime\world_cache_update.wgsl | known-gap(naga internal panic) | hlsl(Compute:sample_di): PANIC[hlsl(Compute:sample_di)]: internal error: entered unreachable code<br>hlsl(Compute:sample_gi): PANIC[hlsl(Compute:sample_gi)]: internal error: entered unreachable code<br>hlsl(Compute:blend_new_samples): PANIC[hlsl(Compute:blend_new_samples)]: internal error: entered unreachable code |

## V4: 无界 binding_array 文件

- crates\bevy_pbr\src\render\parallax_mapping.wgsl: group 3 binding 1, group 3 binding 5
- crates\bevy_solari\src\pathtracer\pathtracer.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\presample_light_tiles.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\realtime_bindings.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\resolve_dlss_rr_textures.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\restir_di.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\restir_gi.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\specular_gi.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\world_cache_compact.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\world_cache_query.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\realtime\world_cache_update.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\scene\brdf.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\scene\raytracing_scene_bindings.wgsl: group 0 binding 0, group 0 binding 1, group 0 binding 2, group 0 binding 3
- crates\bevy_solari\src\scene\sampling.wgsl: group 0 binding 1, group 0 binding 0, group 0 binding 2, group 0 binding 3

## V16: var<immediate> 文件

- crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl: Compute:downsample_depth_first, Compute:downsample_depth_second
- crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl: Compute:clear_visibility_buffer
- crates\bevy_pbr\src\meshlet\cull_bvh.wgsl: Compute:cull_bvh
- crates\bevy_pbr\src\meshlet\cull_clusters.wgsl: Compute:cull_clusters
- crates\bevy_pbr\src\meshlet\cull_instances.wgsl: Compute:cull_instances
- crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl: Compute:remap_dispatch
- crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl: Vertex:vertex, Fragment:fragment
- crates\bevy_pbr\src\render\wireframe.wgsl: Fragment:fragment
- crates\bevy_solari\src\realtime\presample_light_tiles.wgsl: Compute:presample_light_tiles
- crates\bevy_solari\src\realtime\realtime_bindings.wgsl: 
- crates\bevy_solari\src\realtime\restir_di.wgsl: Compute:initial_and_temporal, Compute:spatial_and_shade
- crates\bevy_solari\src\realtime\restir_gi.wgsl: Compute:initial_and_temporal, Compute:spatial_and_shade
- crates\bevy_solari\src\realtime\specular_gi.wgsl: Compute:specular_gi
- crates\bevy_solari\src\realtime\world_cache_update.wgsl: Compute:sample_di, Compute:sample_gi, Compute:blend_new_samples
- crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl: Fragment:fragment

## 全量逐文件结果

| 文件 | compose | SPV | HLSL |
|---|---|---|---|
| crates\bevy_anti_alias\src\contrast_adaptive_sharpening\robust_contrast_adaptive_sharpening.wgsl | ✅ | ✅ 696 words | ✅ 1/1 |
| crates\bevy_anti_alias\src\fxaa\fxaa.wgsl | ✅ | ✅ 2632 words | ✅ 1/1 |
| crates\bevy_anti_alias\src\smaa\smaa.wgsl | ✅ | ✅ 5739 words | ✅ 6/6 |
| crates\bevy_anti_alias\src\taa\taa.wgsl | ✅ | ✅ 2513 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\blit\blit.wgsl | ✅ | ✅ 210 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\deferred\copy_deferred_lighting_id.wgsl | ✅ | ✅ 310 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\fullscreen_vertex_shader\fullscreen.wgsl | ✅ | ✅ 235 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\mip_generation\downsample.wgsl | ✅ | ✅ 5120 words | ✅ 2/2 |
| crates\bevy_core_pipeline\src\mip_generation\experimental\downsample_depth.wgsl | ✅ | ✅ 5249 words | ✅ 2/2 |
| crates\bevy_core_pipeline\src\oit\oit_draw.wgsl | ✅ | ✅ 212 words | — (无 entry point) |
| crates\bevy_core_pipeline\src\oit\resolve\oit_resolve.wgsl | ✅ | ✅ 2585 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\prepass\background_motion_vectors.wgsl | ✅ | ✅ 1382 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\skybox\skybox.wgsl | ✅ | ✅ 1244 words | ✅ 2/2 |
| crates\bevy_core_pipeline\src\tonemapping\lut_bindings.wgsl | ✅ | ✅ 66 words | — (无 entry point) |
| crates\bevy_core_pipeline\src\tonemapping\tonemapping.wgsl | ✅ | ✅ 894 words | ✅ 1/1 |
| crates\bevy_core_pipeline\src\tonemapping\tonemapping_shared.wgsl | ✅ | ✅ 3019 words | — (无 entry point) |
| crates\bevy_dev_tools\src\debug_overlay.wgsl | ✅ | ✅ 1171 words | ✅ 1/1 |
| crates\bevy_dev_tools\src\frame_time_graph\frame_time_graph.wgsl | ✅ | ✅ 970 words | ✅ 1/1 |
| crates\bevy_dev_tools\src\infinite_grid.wgsl | ✅ | ✅ 1649 words | ✅ 1/1 |
| crates\bevy_feathers\src\assets\shaders\alpha_pattern.wgsl | ✅ | ✅ 566 words | ✅ 1/1 |
| crates\bevy_feathers\src\assets\shaders\color_plane.wgsl | ✅ | ✅ 241 words | ✅ 1/1 |
| crates\bevy_gizmos_render\src\line_joints.wgsl | ✅ | ✅ 4155 words | ✅ 4/4 |
| crates\bevy_gizmos_render\src\lines.wgsl | ✅ | ✅ 2469 words | ✅ 4/4 |
| crates\bevy_pbr\src\atmosphere\aerial_view_lut.wgsl | ✅ | ✅ 4321 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\bindings.wgsl | ✅ | ✅ 956 words | — (无 entry point) |
| crates\bevy_pbr\src\atmosphere\bruneton_functions.wgsl | ✅ | ✅ 1533 words | — (无 entry point) |
| crates\bevy_pbr\src\atmosphere\environment.wgsl | ✅ | ✅ 3624 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\functions.wgsl | ✅ | ✅ 6861 words | — (无 entry point) |
| crates\bevy_pbr\src\atmosphere\multiscattering_lut.wgsl | ✅ | ✅ 4210 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\render_sky.wgsl | ✅ | ✅ 6745 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\sky_view_lut.wgsl | ✅ | ✅ 5284 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\transmittance_lut.wgsl | ✅ | ✅ 3194 words | ✅ 1/1 |
| crates\bevy_pbr\src\atmosphere\types.wgsl | ✅ | ✅ 188 words | — (无 entry point) |
| crates\bevy_pbr\src\cluster\cluster.wgsl | ✅ | ✅ 2246 words | — (无 entry point) |
| crates\bevy_pbr\src\cluster\cluster_allocate.wgsl | ✅ | ✅ 2524 words | ✅ 2/2 |
| crates\bevy_pbr\src\cluster\cluster_raster.wgsl | ✅ | ✅ 5767 words | ✅ 2/2 |
| crates\bevy_pbr\src\cluster\cluster_z_slice.wgsl | ✅ | ✅ 4051 words | ✅ 1/1 |
| crates\bevy_pbr\src\decal\clustered.wgsl | ✅ | ✅ 1568 words | — (无 entry point) |
| crates\bevy_pbr\src\decal\forward_decal.wgsl | ✅ | ✅ 11231 words | — (无 entry point) |
| crates\bevy_pbr\src\deferred\deferred_lighting.wgsl | ✅ | ✅ 15595 words | ✅ 2/2 |
| crates\bevy_pbr\src\deferred\pbr_deferred_functions.wgsl | ✅ | ✅ 13642 words | — (无 entry point) |
| crates\bevy_pbr\src\deferred\pbr_deferred_types.wgsl | ✅ | ✅ 1119 words | — (无 entry point) |
| crates\bevy_pbr\src\light_probe\copy.wgsl | ✅ | ✅ 275 words | ✅ 1/1 |
| crates\bevy_pbr\src\light_probe\environment_filter.wgsl | ✅ | ✅ 4574 words | ✅ 2/2 |
| crates\bevy_pbr\src\light_probe\environment_map.wgsl | ✅ | ✅ 2987 words | — (无 entry point) |
| crates\bevy_pbr\src\light_probe\irradiance_volume.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\light_probe\light_probe.wgsl | ✅ | ✅ 1921 words | — (无 entry point) |
| crates\bevy_pbr\src\lightmap\lightmap.wgsl | ✅ | ✅ 839 words | — (无 entry point) |
| crates\bevy_pbr\src\meshlet\clear_visibility_buffer.wgsl | ✅ | ✅ 202 words | ❌ 0/1 |
| crates\bevy_pbr\src\meshlet\cull_bvh.wgsl | ✅ | ✅ 5134 words | ✅ 1/1 |
| crates\bevy_pbr\src\meshlet\cull_clusters.wgsl | ✅ | ✅ 4648 words | ✅ 1/1 |
| crates\bevy_pbr\src\meshlet\cull_instances.wgsl | ✅ | ✅ 3879 words | ✅ 1/1 |
| crates\bevy_pbr\src\meshlet\dummy_visibility_buffer_resolve.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\meshlet\fill_counts.wgsl | ✅ | ✅ 312 words | ✅ 1/1 |
| crates\bevy_pbr\src\meshlet\meshlet_bindings.wgsl | ✅ | ✅ 522 words | — (无 entry point) |
| crates\bevy_pbr\src\meshlet\meshlet_cull_shared.wgsl | ✅ | ✅ 3314 words | — (无 entry point) |
| crates\bevy_pbr\src\meshlet\meshlet_mesh_material.wgsl | ✅ | ✅ 5680 words | ✅ 2/2 |
| crates\bevy_pbr\src\meshlet\remap_1d_to_2d_dispatch.wgsl | ✅ | ✅ 283 words | ❌ 0/1 |
| crates\bevy_pbr\src\meshlet\resolve_render_targets.wgsl | ✅ | ✅ 368 words | ✅ 1/1 |
| crates\bevy_pbr\src\meshlet\visibility_buffer_hardware_raster.wgsl | ✅ | ✅ 3064 words | ❌ 0/2 |
| crates\bevy_pbr\src\meshlet\visibility_buffer_resolve.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\meshlet\visibility_buffer_software_raster.wgsl | ✅ | ✅ 5311 words | ✅ 1/1 |
| crates\bevy_pbr\src\prepass\prepass.wgsl | ✅ | ✅ 1676 words | ✅ 1/1 |
| crates\bevy_pbr\src\prepass\prepass_bindings.wgsl | ✅ | ✅ 135 words | — (无 entry point) |
| crates\bevy_pbr\src\prepass\prepass_io.wgsl | ✅ | ✅ 63 words | — (无 entry point) |
| crates\bevy_pbr\src\prepass\prepass_utils.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\render\build_indirect_params.wgsl | ✅ | ✅ 683 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\clustered_forward.wgsl | ✅ | ✅ 1654 words | — (无 entry point) |
| crates\bevy_pbr\src\render\fog.wgsl | ✅ | ✅ 881 words | — (无 entry point) |
| crates\bevy_pbr\src\render\forward_io.wgsl | ✅ | ✅ 71 words | — (无 entry point) |
| crates\bevy_pbr\src\render\mesh.wgsl | ✅ | ✅ 1668 words | ✅ 2/2 |
| crates\bevy_pbr\src\render\mesh_bindings.wgsl | ✅ | ✅ 177 words | — (无 entry point) |
| crates\bevy_pbr\src\render\mesh_functions.wgsl | ✅ | ✅ 2009 words | — (无 entry point) |
| crates\bevy_pbr\src\render\mesh_preprocess.wgsl | ✅ | ✅ 2182 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\mesh_types.wgsl | ✅ | ✅ 162 words | — (无 entry point) |
| crates\bevy_pbr\src\render\mesh_view_bindings.wgsl | ✅ | ✅ 1161 words | — (无 entry point) |
| crates\bevy_pbr\src\render\mesh_view_types.wgsl | ✅ | ✅ 851 words | — (无 entry point) |
| crates\bevy_pbr\src\render\morph.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\render\occlusion_culling.wgsl | ✅ | ✅ 411 words | — (无 entry point) |
| crates\bevy_pbr\src\render\parallax_mapping.wgsl | ✅ | ✅ 1203 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr.wgsl | ✅ | ✅ 14887 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\pbr_ambient.wgsl | ✅ | ✅ 1503 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_bindings.wgsl | ✅ | ✅ 382 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_fragment.wgsl | ✅ | ✅ 12175 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_functions.wgsl | ✅ | ✅ 12861 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_lighting.wgsl | ✅ | ✅ 6643 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_prepass.wgsl | ✅ | ✅ 152 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\pbr_prepass_functions.wgsl | ✅ | ✅ 71 words | — (无 entry point) |
| crates\bevy_pbr\src\render\pbr_types.wgsl | ✅ | ✅ 907 words | — (无 entry point) |
| crates\bevy_pbr\src\render\reset_indirect_batch_sets.wgsl | ✅ | ✅ 198 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\rgb9e5.wgsl | ✅ | ✅ 654 words | — (无 entry point) |
| crates\bevy_pbr\src\render\shadow_sampling.wgsl | ✅ | ✅ 5096 words | — (无 entry point) |
| crates\bevy_pbr\src\render\shadows.wgsl | ✅ | ✅ 6316 words | — (无 entry point) |
| crates\bevy_pbr\src\render\skinning.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_pbr\src\render\unpack_bins.wgsl | ✅ | ✅ 464 words | ✅ 1/1 |
| crates\bevy_pbr\src\render\utils.wgsl | ✅ | ✅ 2816 words | — (无 entry point) |
| crates\bevy_pbr\src\render\view_transformations.wgsl | ✅ | ✅ 2396 words | — (无 entry point) |
| crates\bevy_pbr\src\render\wireframe.wgsl | ✅ | ✅ 200 words | ✅ 1/1 |
| crates\bevy_pbr\src\ssao\preprocess_depth.wgsl | ✅ | ✅ 1947 words | ✅ 1/1 |
| crates\bevy_pbr\src\ssao\spatial_denoise.wgsl | ✅ | ✅ 1304 words | ✅ 1/1 |
| crates\bevy_pbr\src\ssao\ssao.wgsl | ✅ | ✅ 3021 words | ✅ 1/1 |
| crates\bevy_pbr\src\ssao\ssao_utils.wgsl | ✅ | ✅ 162 words | — (无 entry point) |
| crates\bevy_pbr\src\ssr\raymarch.wgsl | ✅ | ✅ 4910 words | — (无 entry point) |
| crates\bevy_pbr\src\ssr\ssr.wgsl | ✅ | ✅ 19170 words | ✅ 1/1 |
| crates\bevy_pbr\src\transmission\transmission.wgsl | ✅ | ✅ 2927 words | — (无 entry point) |
| crates\bevy_pbr\src\volumetric_fog\volumetric_fog.wgsl | ✅ | ✅ 10286 words | ✅ 2/2 |
| crates\bevy_post_process\src\auto_exposure\auto_exposure.wgsl | ✅ | ✅ 2380 words | ✅ 2/2 |
| crates\bevy_post_process\src\bloom\bloom.wgsl | ✅ | ✅ 1568 words | ✅ 2/2 |
| crates\bevy_post_process\src\dof\dof.wgsl | ✅ | ✅ 2811 words | ✅ 3/3 |
| crates\bevy_post_process\src\effect_stack\chromatic_aberration.wgsl | ✅ | ✅ 705 words | — (无 entry point) |
| crates\bevy_post_process\src\effect_stack\lens_distortion.wgsl | ✅ | ✅ 373 words | — (无 entry point) |
| crates\bevy_post_process\src\effect_stack\post_process.wgsl | ✅ | ✅ 1685 words | ✅ 1/1 |
| crates\bevy_post_process\src\effect_stack\vignette.wgsl | ✅ | ✅ 630 words | — (无 entry point) |
| crates\bevy_post_process\src\gaussian_blur.wgsl | ✅ | ✅ 661 words | — (无 entry point) |
| crates\bevy_post_process\src\motion_blur\motion_blur.wgsl | ✅ | ✅ 1764 words | ✅ 1/1 |
| crates\bevy_render\src\bindless.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_render\src\color_operations.wgsl | ✅ | ✅ 4298 words | — (无 entry point) |
| crates\bevy_render\src\globals.wgsl | ✅ | ✅ 47 words | — (无 entry point) |
| crates\bevy_render\src\maths.wgsl | ✅ | ✅ 1664 words | — (无 entry point) |
| crates\bevy_render\src\occlusion_culling\mesh_preprocess_types.wgsl | ✅ | ✅ 242 words | — (无 entry point) |
| crates\bevy_render\src\render_resource\sparse_buffer_update.wgsl | ✅ | ✅ 573 words | ✅ 1/1 |
| crates\bevy_render\src\view\view.wgsl | ✅ | ✅ 1310 words | — (无 entry point) |
| crates\bevy_render\src\view\window\screenshot.wgsl | ✅ | ✅ 309 words | ✅ 2/2 |
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
| crates\bevy_sprite_render\src\mesh2d\color_material.wgsl | ✅ | ✅ 560 words | ✅ 1/1 |
| crates\bevy_sprite_render\src\mesh2d\mesh2d.wgsl | ✅ | ✅ 353 words | ✅ 2/2 |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_bindings.wgsl | ✅ | ✅ 128 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_functions.wgsl | ✅ | ✅ 965 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_types.wgsl | ✅ | ✅ 89 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_vertex_output.wgsl | ✅ | ✅ 61 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_view_bindings.wgsl | ✅ | ✅ 408 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\mesh2d_view_types.wgsl | ✅ | ✅ 20 words | — (无 entry point) |
| crates\bevy_sprite_render\src\mesh2d\wireframe2d.wgsl | ✅ | ✅ 228 words | ✅ 1/1 |
| crates\bevy_sprite_render\src\render\sprite.wgsl | ✅ | ✅ 1026 words | ✅ 2/2 |
| crates\bevy_sprite_render\src\render\sprite_view_bindings.wgsl | ✅ | ✅ 361 words | — (无 entry point) |
| crates\bevy_sprite_render\src\sprite_mesh\sprite_material.wgsl | ✅ | ✅ 2461 words | ✅ 2/2 |
| crates\bevy_sprite_render\src\tilemap_chunk\tilemap_chunk_material.wgsl | ✅ | ✅ 980 words | ✅ 1/1 |
| crates\bevy_ui_render\src\box_shadow.wgsl | ✅ | ✅ 1930 words | ✅ 2/2 |
| crates\bevy_ui_render\src\gradient.wgsl | ✅ | ✅ 3809 words | ✅ 2/2 |
| crates\bevy_ui_render\src\ui.wgsl | ✅ | ✅ 2541 words | ✅ 2/2 |
| crates\bevy_ui_render\src\ui_material.wgsl | ✅ | ✅ 846 words | ✅ 2/2 |
| crates\bevy_ui_render\src\ui_texture_slice.wgsl | ✅ | ✅ 1648 words | ✅ 2/2 |
| crates\bevy_ui_render\src\ui_vertex_output.wgsl | ✅ | ✅ 63 words | — (无 entry point) |

