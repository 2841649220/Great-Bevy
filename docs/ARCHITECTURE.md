# Great-Bevy 架构设计与系统规格书

本文档系统性阐述 **Great-Bevy** 游戏引擎的整体架构设计、模块划分、数据流动机制以及各核心子系统的工程实现细节。

---

## 1. 架构分层全景

Great-Bevy 采用现代数据驱动（Data-Driven）与微架构设计理念，由上至下划分为四个核心层级：

```text
+-------------------------------------------------------------------------+
|                              应用与逻辑层                                |
|  - 游戏逻辑系统 (Gameplay Systems)                                       |
|  - 场景组织与序列化 (bevy_scene, bevy_gltf)                              |
|  - 交互与窗口控制 (bevy_window, bevy_winit, bevy_input)                 |
+-------------------------------------------------------------------------+
                                    |
                                    v
+-------------------------------------------------------------------------+
|                             核心引擎中枢                                 |
|  - ECS 调度器 (bevy_ecs): Archetype 内存布局、并行 System 调度          |
|  - 空间变换 (bevy_transform): GlobalTransform 层次图求值                |
|  - 资源与资产系统 (bevy_asset): 异步加载、强类型 Handle、热重载           |
+-------------------------------------------------------------------------+
          |                                                   |
          v                                                   v
+-----------------------------------+   +---------------------------------+
|          渲染管线与光照           |   |       厂商插件接口契约层        |
|  - bevy_render: 渲染管线拓扑图    |   |  - bevy_vendor_plugins          |
|  - bevy_pbr: PBR 材质与集群光照   |   |  - DLSS / FSR / XeSS 适配器     |
|  - bevy_solari: 实时光线追踪/GI   |   |  - Reflex / Latency 低延迟链    |
+-----------------------------------+   +---------------------------------+
          \                                                   /
           \                                                 /
            v                                               v
+-------------------------------------------------------------------------+
|                         底层硬件抽象与渲染底座                           |
|  - diligent-rs: 强类型 Rust 安全封装 (Device, Context, Resource, PSO)    |
|  - diligent-sys: 原生 C API Bindgen FFI 与 CMake 原生构建系统            |
|  - Direct3D 12 Ultimate (DX12U) / Vulkan 1.3 驱动层                     |
+-------------------------------------------------------------------------+
```

---

## 2. 核心子系统架构

### 2.1 ECS 调度器 (`bevy_ecs`)
- **Archetype 内存布局**：实体按组件类型组合聚合存储在连续的 `Table` 与 `Archetype` 中，实现极致的 CPU L1/L2 缓存友好度。
- **并行调度器**：基于参数引用借用规则（`&T` 与 `&mut T`）自动构建 Directed Acyclic Graph (DAG)，在多核 CPU 线程池（`bevy_tasks`）上实现无锁自动化并行调度。
- **变更检测 (Change Detection)**：组件通过 `Ref<T>` 与 `Mut<T>` 追踪 ticks，仅在数据被修改时触发相关反应式系统。

---

### 2.2 渲染管线图 (`bevy_render`)
- **主世界与提取世界解耦**：
  - **Main World**：主线程更新游戏逻辑与物理系统。
  - **Extract Phase**：通过 `ExtractSchedule` 将必要的渲染数据（Mesh, Material, Transform, Light）安全拷贝至 `RenderApp`。
  - **Prepare & Queue Phase**：在渲染世界中分配 GPU 缓冲区、构建 BindGroup / ShaderResourceBinding，并排序渲染队列。
  - **Render Phase**：执行 Render Graph 中各个节点（Node），向 GPU 命令列表提交绘制指令。

---

### 2.3 DiligentEngine 后端绑定 (`diligent-sys` / `diligent-rs`)

DiligentEngine 为引擎提供了现代化、跨底层 API 的统一高性能渲染基础设施：

```text
+-----------------------------------+
|            diligent-rs            |  <- 强类型 Rust 包装层 (Safe API)
|  RenderDevice, DeviceContext,     |     - 自动引用计数 (Rc/Arc 风格释放)
|  Buffer, Texture, PipelineState   |     - 类型安全的描述符构建器 (Desc Builder)
+-----------------------------------+
                  |
                  v
+-----------------------------------+
|           diligent-sys            |  <- 原始 C FFI 绑定层 (Raw FFI)
|  bindings.rs (generated)          |     - DiligentCore.lib 静态链接
|  C Vtbl function pointers         |     - CMake + Ninja 离线构建支持
+-----------------------------------+
                  |
                  v
+-----------------------------------+
|         DiligentCore (C++)        |  <- 原生 C++ 驱动交互层
|  D3D12 / Vulkan / Metal 后端实现  |
+-----------------------------------+
```

#### 关键生命周期设计：
1. **设备与上下文创建**：通过 `Diligent_GetEngineFactoryD3D12()` 获取 Direct3D12 引擎工厂，初始化 `IRenderDevice` 与主 `IDeviceContext`。
2. **零开销方法调用**：通过直接解引用 C 虚函数表（Vtbl）指针触发底层硬件指令，杜绝任何中间运行时转换层带来的额外开销。

---

### 2.4 厂商插件架构 (`bevy_vendor_plugins`)

`bevy_vendor_plugins` 采用纯契约（Contract-Driven）设计，不强制捆绑任何专有闭源 SDK：

| 插件接口 | 覆盖技术 | 渲染挂载阶段 |
|----------|----------|--------------|
| `UpscalerPlugin` | DLSS Super Resolution, FSR 2/3, Intel XeSS, ARM ASR, 高通 SGSR | `Core3dSystems::PostProcess` 之后 |
| `FrameGenPlugin` | DLSS FG, AMD FSR Frame Gen, Intel XeSS FG | Swapchain Present 代理阶段 |
| `DenoiserPlugin` | DLSS Ray Reconstruction (RR), FSR Ray Regeneration | DXR 光追加速结构计算输出后 |
| `LatencyPlugin` | NVIDIA Reflex, AMD Anti-Lag 2, Intel XeLL | 帧起始与输入采样边界 |
| `PureAaPlugin` | DLAA, FSR Native AA | 抗锯齿 Pass |

- **探测机制 (Probe Protocol)**：启动时通过 `DirectoryProbe` 扫描 `plugins/<backend>/<vendor>/` 目录（例如检测 `sdk-version.txt`）。若检测到厂商 SDK 则自动加载对应适配器；若未检测到，引擎自动平滑回退至内置的 TAA/FXAA/SMAA 算法路径。

---

### 2.5 Solari 实时光线追踪与全局光照 (`bevy_solari`)
- **TLAS & BLAS 管理**：
  - **BLAS (Bottom-Level Acceleration Structure)**：为每个网格几何体构建硬件光线追踪包围盒层次树（BVH）。
  - **TLAS (Top-Level Acceleration Structure)**：按帧收集场景中所有活动实体的 Transform 矩阵，动态更新顶层实例表。
- **光线追踪着色器管线**：支持 Ray Generation Shader、Closest Hit Shader、Any Hit Shader 与 Miss Shader，支持 Primary Ray、Shadow Ray、Reflection 与 Ambient Occlusion 多通路求交计算。

---

## 3. 数据流与每帧生命周期

```text
[Frame Start]
      │
      ├─► 1. 延迟标记采集 (LatencyPlugin Marker)
      │
      ├─► 2. 输入事件处理 (Input / Window Events)
      │
      ├─► 3. 主逻辑与 ECS 系统更新 (Main World Update Schedule)
      │       ├─ 空间层次变换求值 (Transform Propagation)
      │       └─ 动画与动力学更新 (Animation & Dynamics)
      │
      ├─► 4. 提取阶段 (Extract Phase to RenderApp)
      │
      ├─► 5. 加速结构更新 (TLAS/BLAS Rebuild / Refit)
      │
      ├─► 6. 渲染管线调度 (Render Graph Execution)
      │       ├─ G-Buffer / Depth Prepass
      │       ├─ DXR 光线求交与 Solari 光追计算
      │       ├─ Denoise 降噪通道 (DenoiserPlugin / DLSS RR)
      │       ├─ Forward / Deferred PBR Lighting
      │       ├─ AI 超分辨率放大 (UpscalerPlugin / DLSS / FSR / XeSS)
      │       └─ HUD / UI 合成渲染
      │
      ├─► 7. 帧生成 (FrameGenPlugin / DLSS FG)
      │
      └─► 8. 呈现与交换链提交 (Swapchain Present)
[Frame End]
```
