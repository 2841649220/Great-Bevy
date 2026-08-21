# 参考帧采集方法论（M0 Reference-Frame Capture Methodology）

Project: rendering-backend-replacement (Bevy 0.19 fork, M0 phase)
Sources: 施工方案 §4.5.1（工具链搭建）、§4.5.2（首轮采集）、§11.4（采集施工法）
Tool: `tools/reference_frames/`（crate `reference_frames`）

## 1. 目的与时机（Purpose & Timing）

- 参考帧（reference frames）是从**当前 wgpu 29.0.4 路径**采集的静态基准资产，用于 M2a 起对照替换后渲染器（Diligent）的输出。
- **硬性约束**：必须在 M1 移除 wgpu 之前完成采集。本工具与本文档即为该采集链。
- 本机（Windows）采集属首轮尽力而为部分；**Linux 主参照采集需真 GPU runner（RTX 3060 级；lavapipe/llvmpipe 不采）**，标记为待 CI 真 GPU runner，文档先行。

## 2. 参考分层与白名单档位（Reference Tiers & Whitelist）

| 层 | 说明 | 判定 |
|---|---|---|
| **Linux 同后端主参照** | wgpu-Vulkan → Diligent-Vulkan，两侧同为 naga→SPIR-V 字节码 + 同一驱动，差异仅来自命令序列/状态管理（render pass 结构、descriptor 布局、depth bias 设置），即"封装差异"而非"后端差异" | SSIM ≥ 0.99 |
| **Windows 白名单档位（预置）** | 参考帧来自 wgpu-Vulkan，替换后为 Diligent-D3D12。**预置 D3D12 档位差异白名单**：光栅化规则（rasterization rules）、深度范围（depth range）、采样坐标（sample coordinates）、blend 精度（blend precision） | 阈值放宽，须落在白名单内 |

### 白名单管理流程（§11.4）

1. 对照运行 `compare` 得到 SSIM/PSNR/像素级差分布（直方图）。
2. **系统性差异**（后端特性所致，如 D3D12 深度范围 [0,1] 约定、光栅化规则、blend 精度）→ **入白名单**，需在本文档 §8 记录理由与范围。
3. **随机差异**（逐帧不一致/抖动）→ **一律排查**，可能是 bug（状态管理、编译器、驱动）。
4. 状态类差异可白名单；**采样/浮点类差异一律排查**（§10.3 原则）。
5. 超出白名单 → 应急：排查差异来源修复至白名单内；第二应急：扩展白名单（仅限系统性差异，必须文档化理由，§13.2.5）。

## 3. 输出布局（Output Layout）

```
tests/reference-frames/
├── README.md                       ← 本文档（方法论）
├── <platform>/                     ← std::env::consts::OS（windows / linux / macos）
│   └── <scene>/
│       ├── frame_0000.png          ← 8-bit RGB 颜色截图
│       ├── frame_0000.exr          ← float RGB 颜色数据（EXR）
│       ├── depth_0000.exr          ← 真实深度缓冲（Depth32Float 读回，见 §5.1）
│       ├── frame_0000.json         ← 元数据 sidecar（后端来源/驱动版本/分辨率/帧号/时间/深度元数据）
│       ├── frame_0001.{png,exr,json} + depth_0001.exr
│       └── ...
```

- 每帧四件套齐全；JSON 是后端归属的唯一依据（见 §5 字段表）。
- 相同场景跨平台同名同帧，直接 `compare` 对照。
- `legacy-msaa/`（Task 12 起）：Task 5 首轮采集的旧三件套（PNG+颜色 EXR+JSON，MSAA Sample4、无深度 EXR）全部移至
  `<platform>/legacy-msaa/<scene>/` 作历史参照，**不是** M2a 对照资产；对照一律以 `<platform>/<scene>/` 四件套为准。

## 4. 确定性策略（Determinism Policy）

1. **固定相机**：每场景注册表内固定 camera rig（位置 + look-at），无输入/相机控制器。
2. **固定步进**：手动帧计数器驱动；每渲染帧推进一次 `DeterministicClock`（固定 1/60 delta）。场景动画必须以 `DeterministicClock` 为唯一时间源，不得读取 `Time`。
3. **固定种子**：场景内伪随机一律用固定 key 的 hasher（如 `DefaultHasher` over 固定坐标，与 bloom_3d 原例一致）。
4. **无时域特效**：TAA 等时域抖动禁用（ssao 场景已移除 TAA）。
5. 输出窗口固定 1280×720（可 `--size WxH` 覆盖）；present mode 不影响像素输出（截图取自渲染目标纹理）。
6. **管线预热（重要）**：PBR 等管线在 debug 构建下**异步编译**，本机实测需约 40–60 秒墙钟时间才出现内容帧；在此之前捕获到的帧为**纯色帧**（仅 clear color）。对策：默认 `--warmup 200`（≈本机 debug 编译时间），每帧 JSON 记 `image_uniform` 标志（纯色帧为 `true`，不可作为参照内容，需加大 warmup 重采）；深度帧同样以 `depth.uniform` 标志标记。发布构建下编译快得多，可减小 warmup。
7. **跨运行可复现已验证**：同参数两次独立运行，颜色帧与深度帧均逐字节一致（SSIM=1.0 / PSNR=inf / max_abs_diff=0；深度 EXR SHA-256 相同）。
8. **MSAA 强制 Off（自 Task 10 起）**：深度回读需要单采样深度纹理（多采样纹理不可 texel-copy），工具在 `PostStartup` 强制所有 3D 相机 `Msaa::Off` 并为 `Camera3d.depth_texture_usages` 增加 `COPY_SRC`（公开 API，工具层注入，不改 bevy 源码）。**因此颜色输出与首轮（默认 Sample4）采集的边缘抗锯齿不同**；跨后端对照本就要求确定性光栅化，后续复采均以 Msaa::Off 为准（ssao 场景本就 Msaa::Off）。

## 5. 元数据 sidecar（JSON 字段）

| 字段 | 含义 |
|---|---|
| `tool` / `tool_version` / `bevy_version` / `wgpu_version` | 工具与引擎归属（wgpu_version = 29.0.4，采集路径） |
| `platform` / `arch` | 采集环境 |
| `scene` / `frame` | 场景 id 与帧号 |
| `width` / `height` / `surface_logical_size` / `scale_factor` | 分辨率（物理表面尺寸与逻辑尺寸） |
| `pixel_format` | 截图 Image 的纹理格式 |
| `captured_at_unix_secs` / `captured_at_utc` | 采集时间 |
| `adapter` | **后端来源**：`name`（adapter 名）、`vendor`、`device`、`device_type`、`driver`、`driver_info`、`backend`（Vulkan / D3D12 / GL…），取自主世界 `RenderAdapterInfo`（wgpu AdapterInfo） |
| `files` | 本帧四件套文件名（`depth` 为深度 EXR，缺失时 `null`） |
| `depth` | 深度元数据（见 §5.1）：`file`、`format`、`value_range`、`projection`、`source_pass`、`msaa`、`clear_value`、`uniform`（全像素相同 = 管线未就绪的纯深度帧）、`stats`（min/max/mean/non_finite） |
| `determinism` | 确定性配置说明 |

## 5.1 深度 EXR（depth_<n>.exr）

- **内容**：主 3D pass 的真实深度纹理（`ViewDepthTexture`，`Depth32Float`），由工具侧渲染图节点在 MainPass 之后直接 texel-copy 到 `MAP_READ` 缓冲，再异步 map 读回（wgpu 29 允许 Depth32Float 作为拷贝源；无需格式转换 pass）。深度读回与截图各自异步解析，JSON 在两者都落盘后写出；深度失败时捕获整体报错退出（exit 1），不静默产出缺深度帧。
- **文件格式**：Rgb32F EXR，**R=G=B=深度值**（image crate 的 EXR 编码器仅支持 Rgb32F/Rgba32F 输出；三通道冗余便于现有 `compare`/查看器直接读）。
- **值域约定（重要）**：Bevy 0.19 默认 `PerspectiveProjection` 使用**无限远 reverse-Z**（`perspective_infinite_reverse_rh`，near=0.1）：
  - `1.0` = 近平面（0.1），`0.0` = 远平面/天空背景（深度纹理 clear 值即 0.0，见 `Camera3dDepthLoadOp::Clear(0.0)`）；
  - 中间值为 `near / 视距`（线性于 1/距离）。**小值 = 远，大值 = 近**（与 D3D 传统 [0,1] near=0 约定相反，对比 Diligent-D3D12 输出时注意转换）。
- **实测校准（3d_scene，1280×720，Vulkan）**：天空像素恰为 0.0；画面中心像素（立方体前表面，视距约 9.8m）≈ 0.0102 ≈ 0.1/9.8；全帧最大值 ≈ 0.01475 ≈ 0.1/6.78m（地面圆盘最近可见点）。深度 EXR 确为真实深度缓冲。

## 6. 场景集与种子清单（Scene / Seed List）

场景注册表：`reference_frames scenes`。首轮实现（程序化静态重建，无需资产文件）：

| 场景 | 状态 | 相机 rig（位置 → 目标） | 说明 |
|---|---|---|---|
| `3d_scene` | ✅ 已复采（Task 12，四件套 + Msaa::Off，30 帧） | (-2.5, 4.5, 9) → (0,0,0) | 与 examples/3d/3d_scene.rs 一致；首轮三件套已移至 `legacy-msaa/` |
| `pbr` | ✅ 已采集（Task 12，30 帧） | (4, 3, 6) → (0, 0.5, 0) | 4×4 metallic×roughness 球阵 + 平面 + 平行/点光 |
| `lighting` | ✅ 已采集（Task 12，30 帧） | (0, 3, 8) → (0, 0.5, 0) | 平面 + 球 + 平行/彩色点光/聚光 |
| `bloom_3d` | ✅ 已复采（Task 12，四件套 + Msaa::Off，30 帧） | (-2, 2.5, 5) → (0,0,0) | Bloom::NATURAL + HDR 自发光球阵（hash 种子同原例）；bounce 动画移除；首轮三件套已移至 `legacy-msaa/` |
| `ssao` | ✅ 已采集（Task 12，30 帧） | (-2, 2, -2) → (0,0,0) | 与 examples/3d/ssao.rs 一致；TAA 移除（时域）；本就 Msaa::Off + Hdr + DepthPrepass（prepass 深度读回已验证） |
| `lightmaps` | ⏳ 未实现（Task 12 尝试：exit 3） | (-2.5, 4.5, 9) → (0,0,0) | 需 glTF + 烘焙 lightmap 资产；fork `assets/` 内无 `lightmap_example.gltf`，需另备资产 |
| `irradiance_volumes` | ⏳ 未实现（Task 12 尝试：exit 3） | (4, 3, 6) → (0,0,0) | 需 irradiance volume 资产管线；fork `assets/` 内无对应 glTF |
| `deferred_rendering` | ⏳ 未实现（Task 12 尝试：exit 3） | (-2.5, 4.5, 9) → (0,0,0) | 需重建 deferred 管线展示 |
| `meshlet` | ⏳ 未实现（Task 12 尝试：见 §8.4） | (10,10,10) → (0,0,0) | 场景未实现（exit 3 路径）；`--features meshlet,https,free_camera` 对 reference_frames 包被 cargo 拒绝（bevy 包 feature，需工具 Cargo.toml 转发）；`free_camera` 输入控制器与确定性策略冲突；需网络下载网格 |
| 压力档 | ⏳ 文档化 | — | `large_scenes`：bistro / caldera_hotel，仅文档化，不入首轮 |

- 种子：场景重建全部确定性；`bloom_3d` 的球阵颜色用 `DefaultHasher` over (x,z)（与原例相同 seed 模式）。动画种子策略（未来）：以 `DeterministicClock` 时间为输入，固定推导函数。
- 未实现场景由工具显式报错退出（exit 3），不静默。

## 7. 复现命令（Reproduction Commands）

```powershell
# 场景清单
cargo run -p reference_frames -- scenes

# 采集（Windows）：3d_scene，30 帧，默认预热 200 帧，1280x720
# 产出四件套：frame_<n>.png / frame_<n>.exr（颜色）/ depth_<n>.exr（真实深度）/ frame_<n>.json
cargo run -p reference_frames -- capture 3d_scene --frames 30 --warmup 200 --size 1280x720
# 输出：tests/reference-frames/windows/3d_scene/frame_0200..0229.{png,exr,json} + depth_0200..0229.exr

# 强制后端（wgpu 默认即读 WGPU_BACKEND 环境变量）
$env:WGPU_BACKEND = "vulkan"   # 或 "dx12"
cargo run -p reference_frames -- capture 3d_scene --frames 30

# 全部已实现场景（Task 12 已按此参数采集，帧 0200..0229）
cargo run -p reference_frames -- capture pbr --frames 30
cargo run -p reference_frames -- capture lighting --frames 30
cargo run -p reference_frames -- capture bloom_3d --frames 30
cargo run -p reference_frames -- capture ssao --frames 30

# 对照：SSIM / PSNR / 像素级差直方图（+inf 序列化为 "inf"）
cargo run -p reference_frames -- compare tests/reference-frames/windows/3d_scene/frame_0210.png <candidate>.png

# 深度 EXR 校验：统计 + 采样像素（--pixel 可重复；1.0=近平面，0.0=远/天空）
cargo run -p reference_frames -- depth-stats tests/reference-frames/windows/3d_scene/depth_0210.exr --pixel 640,360 --pixel 640,10

# 单元测试（SSIM/PSNR/直方图/时间格式化/深度去 padding/统计/作业状态机）
cargo test -p reference_frames
```

Linux 主参照（待真 GPU runner，文档先行）：

```bash
cargo run -p reference_frames -- capture 3d_scene --frames 30   # 输出 tests/reference-frames/linux/3d_scene/
```

**注意**：Task 5 首轮采集的 `windows/{3d_scene,bloom_3d}` 旧三件套（无 `depth_<n>.exr`，MSAA Sample4 颜色）已于 **Task 12 复采为四件套 + Msaa::Off 并移至 `windows/legacy-msaa/`**；深度对照（P1）一律使用当前 `windows/<scene>/` 下的四件套。

## 8. 已知差距与后续工作（Known Gaps & Follow-ups）

1. **Linux 采集**：需 CI/runner 真 GPU（RTX 3060 级）；lavapipe/llvmpipe 不采（§11.4）。本机无 Linux 环境。
2. **深度缓冲 EXR（Task 10 已实现）**：真实 Depth32Float 深度读回已落地（§5.1），取代原"颜色 float 拷贝当深度"的临时方案；颜色 float 拷贝 EXR（`frame_<n>.exr`）保留，供浮点颜色级对照（P1 可能比对颜色 EXR 的浮点值），与 `depth_<n>.exr` 并存，不破坏既有产物命名。
3. **首轮旧产物已复采（Task 12 完成）**：`windows/{3d_scene,bloom_3d}` 已按 §7 命令复采为四件套 + Msaa::Off（帧 0200..0229）；旧三件套（Task 5，MSAA Sample4）已移至 `windows/legacy-msaa/` 作历史参照（原命名与复采帧同名，无法原位共存）。**对照时勿用 legacy-msaa/ 资产。**
4. **未实现场景（Task 12 尝试，均 exit 3「registered but not implemented」）**：
   - `lightmaps` / `irradiance_volumes` / `deferred_rendering`：注册未实现（`scenes.rs` `implemented: false`），且 fork `assets/` 缺少 lightmap/irradiance volume glTF 资产，采集不可行——需后续任务实现场景 + 备齐资产。
   - `meshlet`：见 §8.4。
5. **压力档**：large_scenes（bistro/caldera_hotel）仅文档化。
6. **白名单文件**：M2a 对照开始时在 `tests/reference-frames/` 下建立 `whitelist-<backend-pair>.json`（每场景每帧允许的 SSIM/PSNR/直方图档位），系统性差异入档并在此文档记录理由。
7. 截图 alpha 通道按 Bevy `save_to_disk` 惯例丢弃（HDR 时 alpha 存亮度）。

### 8.4 meshlet 场景专项（Task 12）

按简报命令 `cargo run -p reference_frames --features meshlet,https,free_camera -- capture meshlet ...` 实测结果：

1. **cargo 直接拒绝**（exit 101）：`the package 'reference_frames' does not contain these features: free_camera, https, meshlet`——这些是 **bevy 包**的 feature（cargo 提示存在于 `bevy_asset`/`bevy_pbr`/`bevy_internal`/`bevy`/`bevy_camera_controller`），reference_frames 的 Cargo.toml 未声明转发（`bevy = { features = ["3d","default_platform","exr"] }`）。要启用需在工具 Cargo.toml 的 bevy 依赖上追加这三个 feature（未做——超出本任务最小修复范围）。
2. **即使 feature 构建成功也无法采集**：`scenes.rs` 中 meshlet 场景 `implemented: false`、`setup: None`，capture 直接走 exit 3 路径（与 §8 其他未实现场景一致）。
3. **确定性冲突**：场景要求 `free_camera`（用户输入相机控制器），与 §4 确定性策略（固定相机 rig、无输入）根本冲突；如需采集须另写固定相机版本。
4. **资产依赖**：原例需运行时从网络下载高模网格（`https` feature），采集机须联网且受下载确定性影响。

结论：meshlet 场景登记为遗留，需后续任务（实现固定相机场景 + 工具 Cargo.toml 加 feature + 资产就绪）后另行采集。

## 9. 采集环境记录（First-Round Capture Environment）

- 平台：Windows 11（本机）；后端以运行时为准（wgpu-Vulkan 或 wgpu-D3D12），逐帧 JSON 记录 `adapter.backend`。
- 硬件：NVIDIA GeForce RTX 3050 Ti Laptop GPU（驱动 610.74，见 sidecar `adapter.driver_info`）。
- 首轮已采：`3d_scene`（30 帧）与 `bloom_3d`（30 帧），帧号 0200..0229，1280×720，后端 Vulkan；内容帧已确认非纯色（`image_uniform=false`）。**Task 12 起移至 `windows/legacy-msaa/`（历史参照）。**
- Task 10 实测（深度回读验证）：`3d_scene` 3 帧（帧 0200..0202，1280×720，Vulkan），`depth_0200.exr` 统计 min=0.0 / max≈0.01475 / mean≈0.00221，`uniform=false`，两独立运行 SHA-256 逐字节一致；中心像素（立方体前表面）≈0.0102。
- **Task 12 复采/采集（四件套 + Msaa::Off，1280×720，Vulkan，warmup 200，帧 0200..0229，每帧 4 文件）**：
  - `3d_scene`（复采）：depth_0210 min=0.0 / max=0.014747 / mean≈0.00221，中心 (640,360)=0.0102（立方体前表面 ~9.8m），(640,10)=0.0（天空），与 Task 10 校准一致。
  - `bloom_3d`（复采）：depth max=0.037669（最近大球 ~2.65m），中心 (640,360)=0.0187，(640,180)=0.0116，(400,300)=0.0152，(640,10)=0.0。
  - `pbr`（新采）：depth_0210 max=0.027817（最近球面前缘 ~3.59m），中心 (640,360)=0.01588，天空 0.0。
  - `lighting`（新采）：depth_0210 max=0.023103（最近几何 ~4.33m），中心 (640,360)=0.01129，天空 0.0。
  - `ssao`（新采）：depth_0210 max=0.032635，中心 (640,360)=0.03263 ≈ 0.1/3.06m = 原点球（r=0.4）前表面（相机至原点 3.46m），天空 0.0——**DepthPrepass 深度读回验证通过**（Task 10 遗留项 3 关闭）。
  - 全部 150 帧 JSON：`depth.file` 齐全、`image_uniform=false`、`adapter.backend=Vulkan`、`depth.msaa="Off"`。
- 复现命令：`cargo run -p reference_frames -- capture <scene> --frames 30 --warmup 200 --size 1280x720`。
- 磁盘占用：每帧约 22 MB（颜色 EXR + 深度 EXR 各约 11 MB 未压缩 Rgb32F + PNG + JSON）；压力档采集建议 `--size 640x360` 或发布构建。
