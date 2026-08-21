# Great-Bevy 功能特性与技术矩阵白皮书

本文档系统性介绍 **Great-Bevy** 引擎所集成的核心技术特性、图形功能支持矩阵以及未来路线规划。

---

## 1. DirectX 12 Ultimate (DX12U) 四大技术支柱

DirectX 12 Ultimate 是现代图形硬件（NVIDIA RTX 20/30/40/50 系列、AMD RDNA 2/3/4 系列、Intel Arc 系列）的统一旗舰级标准。Great-Bevy 原生深度接入了 DX12U 的全部四大核心特性：

| 特性支柱 | 引擎支持等级 | 核心机制与优势 | 典型应用场景 |
|----------|--------------|----------------|--------------|
| **DirectX Raytracing (DXR 1.1)** | 原生支持 (Inline + Dispatch) | 硬件加速 BVH（TLAS/BLAS）、内联光线求交（Ray Query）、动态着色器表（SBT） | 物理精确阴影、真实镜面反射、环境光遮蔽 (RTAO)、实时全局光照 |
| **Mesh Shaders & Amplification** | 原生支持 | 基于 GPU 算力的几何两阶段管线，彻底消除 CPU DrawCall 与传统顶点/几何着色器瓶颈 | 复杂星体表面密集几何剔除、LOD 细分、大世界植被与碎石网格化渲染 |
| **Variable Rate Shading (VRS Tier 2)** | 原生支持 | 支持逐图元与图像驱动（Screen-space image）着色率调节（1x1, 1x2, 2x1, 2x2, 4x4 等） | 动态模糊区域着色降频、周边视线注视点渲染、大幅提升高分辨率帧率 |
| **Sampler Feedback** | 原生支持 | 记录与分析纹理采样命中区域，最小化显存加载 | 超大规模虚无纹理（Virtual Texturing）、材质流式加载（Streaming） |

---

## 2. 厂商超分辨率与图形增强插件矩阵 (`bevy_vendor_plugins`)

Great-Bevy 提供了业界首创的**无捆绑、零硬依赖**厂商插件扩展架构：

```text
               +-----------------------------------+
               |        bevy_vendor_plugins        |
               +-----------------------------------+
                                 |
        +------------------------+------------------------+
        |                        |                        |
        v                        v                        v
+----------------+      +----------------+      +----------------+
| NVIDIA 适配器  |      |   AMD 适配器   |      |  Intel 适配器  |
| - DLSS 3.7 SR  |      | - FSR 3.1 SR   |      | - XeSS 1.3 SR  |
| - DLSS FG      |      | - FSR FrameGen |      | - XeSS FG      |
| - DLSS 3.5 RR  |      | - Anti-Lag 2   |      | - XeLL Low Lat |
| - Reflex SDK   |      | - FSR NativeAA |      |                |
+----------------+      +----------------+      +----------------+
```

### 2.1 支持特性一览

1. **超分辨率 (Super Resolution)**：
   - 统一输入接口：Color、Linear Depth、Motion Vectors、Jitter Phase、Reactive Mask。
   - 质量档位：Native AA、Quality（质量）、Balanced（平衡）、Performance（性能）、Ultra Performance（极致性能）。
2. **光追降噪 (Ray Reconstruction / Denoising)**：
   - 传递多通道原始未降噪光追信号（Direct/Indirect Diffuse, Specular, AO, Specular Occlusion）。
   - 由 AI 降噪模型替代传统双边滤波，保留极高光追细节。
3. **帧生成 (Frame Generation)**：
   - 光流插帧计算（Optical Flow），配合 Proxy Swapchain 实现帧率翻倍。
4. **超低系统延迟 (System Latency Reduction)**：
   - 动态调控 CPU 与 GPU 渲染队列，消除输入轮询与画面呈现之间的管道堆积。

---

## 3. Solari 实时全局光照系统 (`bevy_solari`)

- **混合渲染路径 (Hybrid Rendering Pipeline)**：G-Buffer 光栅化 + DXR 硬件光线求交。
- **空间加速结构自动化管理**：
  - 每帧自动化收集具有 `Transform` 与 `Mesh` 组件的活动实体。
  - 动态 Refit / Rebuild 顶层加速结构（TLAS）。
- **材质系统集成**：深度兼容 PBR 材质（Roughness, Metallic, Emissive, Normal Maps）。

---

## 4. 跨平台支持与后端规划

| 平台 / 后端 | 当前状态 | 目标 API | 说明 |
|-------------|----------|----------|------|
| **Windows 10 / 11** | ✅ 完整支持 | Direct3D 12 (DX12U) / Vulkan 1.3 | 主力开发与测试验证平台 |
| **Linux (Desktop)** | 🔄 规划适配 | Vulkan 1.3 / SPIR-V | 基于 `diligent-rs` 的跨平台 Vulkan 路径 |
| **Android (Mobile)**| 🔄 规划适配 | Vulkan 1.2+ / OpenGLES | 支持 ARM ASR 与高通 Snapdragon 超分辨率 |
