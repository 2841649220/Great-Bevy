# reference_frames — M0 参考帧采集工具（Reference-Frame Capture Toolchain）

**规范文档（方法论、白名单档位、场景/种子清单、复现命令）：`tests/reference-frames/README.md`** — 本文档为工具速览。

## 用途

在 M1 移除 wgpu 之前，从当前 wgpu 29.0.4 fork 采集确定性参考帧（PNG + 颜色 EXR + **深度 EXR** + JSON 元数据），
供 M2a 起对照替换后的渲染器（Diligent）输出。仅经 Bevy 公开 API 叠加（plugins/systems），不改任何 fork 源码。

## 快速上手

```powershell
cargo run -p reference_frames -- scenes                                   # 场景注册表
cargo run -p reference_frames -- capture 3d_scene --frames 30 --warmup 200  # 采集（含深度 EXR）
$env:WGPU_BACKEND = "dx12"                                                # 强制后端（wgpu 原生环境变量）
cargo run -p reference_frames -- compare <ref.png> <candidate.png>        # SSIM/PSNR/直方图
cargo run -p reference_frames -- depth-stats <depth_0200.exr> --pixel 640,360  # 深度 EXR 校验
cargo test -p reference_frames                                            # 度量/深度模块单测
```

输出：`tests/reference-frames/<platform>/<scene>/frame_<n>.png|.exr|.json` + `depth_<n>.exr`（真实深度，R=G=B=深度值；
值域 [0,1]，1.0=近平面 / 0.0=远，reverse-Z，详见规范文档 §5.1）。

## 方法论要点（§11.4）

- **Linux 同后端主参照**：wgpu-Vulkan → Diligent-Vulkan，两侧同为 naga→SPIR-V 字节码 + 同一驱动，
  差异仅来自命令序列/状态管理（render pass 结构、descriptor 布局、depth bias），即"封装差异"；SSIM ≥ 0.99。
- **Windows 白名单档位（预置）**：参考帧来自 wgpu-Vulkan，替换后为 Diligent-D3D12；
  预置 D3D12 档位差异白名单（光栅化规则、深度范围、采样坐标、blend 精度），阈值放宽。
- **白名单管理**：系统性差异（后端特性）→ 入白名单（文档化理由）；随机差异（逐帧抖动）→ 一律排查。
  状态类差异可白名单，采样/浮点类差异一律排查。
- **确定性**：固定相机 rig、手动帧步进（`DeterministicClock` 固定 1/60 delta）、固定种子、无时域特效；
  跨运行逐字节一致（已实测 SSIM=1.0，深度 EXR 亦逐字节一致）。
- **深度 EXR（Task 10）**：工具在 `PostStartup` 强制相机 `Msaa::Off` 并为 `Camera3d.depth_texture_usages`
  增加 `COPY_SRC`，经 Core3d 渲染图节点把 `ViewDepthTexture`（Depth32Float）texel-copy 到 `MAP_READ` 缓冲后
  异步 map 读回，写为 Rgb32F EXR（R=G=B=深度）。值域 [0,1]：1.0=近平面、0.0=远（reverse-Z，near=0.1）。
  注意：强制 Msaa::Off 后颜色输出与首轮（Sample4）不同，详见规范文档 §5.1。
- **管线预热**：PBR 管线 debug 下异步编译约 40–60s；默认 `--warmup 200`；纯色帧由 JSON `image_uniform`/`depth.uniform` 标志识别。

## 场景

已实现（程序化静态重建）：`3d_scene`、`pbr`、`lighting`、`bloom_3d`、`ssao`（**Task 12 已全部采集**：四件套 + Msaa::Off，各 30 帧，帧 0200..0229，见规范文档 §9）。
已注册未实现：`lightmaps`、`irradiance_volumes`、`deferred_rendering`、`meshlet`（Task 12 已尝试，均 exit 3；meshlet 的 `--features meshlet,https,free_camera` 被 cargo 拒绝——bevy 包 feature 未在工具 Cargo.toml 转发，详见规范文档 §8/§8.4）。
