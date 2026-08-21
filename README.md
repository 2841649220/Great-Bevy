# Great-Bevy (Next-Gen DirectX 12 Ultimate & DiligentEngine Game Engine)

<div align="center">

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/edition-2024-blue.svg)](https://doc.rust-lang.org/edition-guide/rust-2024/index.html)
[![Graphics](https://img.shields.io/badge/DirectX-12%20Ultimate%20(DX12U)-0078D7.svg)](https://developer.microsoft.com/en-us/windows/hardware/)
[![RayTracing](https://img.shields.io/badge/Ray%20Tracing-DXR%201.1%20%7C%20Solari-green.svg)](https://microsoft.github.io/DirectX-Specs/d3d/Raytracing.html)
[![Backend](https://img.shields.io/badge/Backend-DiligentEngine%20%2B%20WGPU-9cf.svg)](https://github.com/DiligentGraphics/DiligentEngine)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-lightgrey.svg)](LICENSE-MIT)

**Great-Bevy** 是一套基于 [Bevy](https://bevy.org) 深度重构与现代化升级的次世代数据驱动游戏引擎与高并发图形计算框架。项目深度融合了 **DirectX 12 Ultimate (DX12U)** 硬件特性、**DiligentEngine** 跨平台现代化渲染底座、**Solari** 硬件加速光线追踪/全局光照体系，以及面向工业级落地的**厂商插件体系（NVIDIA DLSS / AMD FSR / Intel XeSS / Reflex / ARM ASR 等）**。

[English](#overview-en) • [核心特性](#-核心特性) • [架构全景](#-架构全景) • [快速启动](#-快速启动与运行) • [演示 Demo](#-演示-demo) • [工程文档](#-工程文档导览) • [开源协议](#-开源协议)

</div>

---

## 🌟 核心特性

### 1. 🚀 DirectX 12 Ultimate (DX12U) 四大技术支柱
* **DirectX Raytracing (DXR 1.1)**：支持内联光线追踪（Inline Raytracing / Ray Queries）与 DispatchRays 计算调度，动态构建 TLAS（顶层加速结构）与 BLAS（底层加速结构）。
* **网格着色器 (Mesh Shaders / Amplification Shaders)**：现代化两阶段几何处理管线，彻底替代传统 VS/GS 阶段，突破多边形与 LOD 瓶颈。
* **可变速率着色 (Variable Rate Shading - VRS)**：支持 Tier 2 逐图元与图像驱动（Screen-space image）VRS，显著降低重度像素着色负载。
* **采样器反馈 (Sampler Feedback)**：流式纹理加载与显存空间动态评估，消除高精度纹理显存冗余。

### 2. ⚡ 现代化多后端图形渲染底座 (DiligentEngine + WGPU)
* **`diligent-sys` / `diligent-rs`**：提供与 [DiligentCore](https://github.com/DiligentGraphics/DiligentEngine) 的零开销 `extern "C"` FFI 绑定与符合 Rust 安全人体工学的强类型安全封装（Device、Context、Swapchain、Pipeline State、Shader Resource Binding 等）。
* **自动化子模块回填与校验**：内置 CMake + Ninja 原生混合构建脚本，支持离线包完整性校验与平台自动探测。

### 3. 🧩 工业级厂商插件扩展框架 (`bevy_vendor_plugins`)
* **五类标准插件契约**：
  * `UpscalerPlugin`：DLSS Super Resolution、AMD FSR 2/3、Intel XeSS、ARM ASR、高通 SGSR。
  * `FrameGenPlugin`：DLSS Frame Generation、FSR FG、XeSS FG 帧生成集成。
  * `DenoiserPlugin`：DLSS Ray Reconstruction (RR)、FSR Ray Regeneration 光追降噪通道。
  * `LatencyPlugin`：NVIDIA Reflex、AMD Anti-Lag 2、Intel XeLL 延迟优化协议。
  * `PureAaPlugin`：DLAA、FSR Native AA 原生抗锯齿。
* **优雅降级与热插拔**：无厂商 SDK 时引擎自动降级至内置 TAA/FXAA/SMAA/CAS 渲染路径，零外部硬依赖。

### 4. ☀️ Solari 实时全局光照与硬件光线追踪
* 包含实时光线追踪管线（Realtime GI / Path Tracer）、SDF 符号距离场与高精度天体空间 PBR 材质系统。

---

## 🏛️ 架构全景

```text
+-----------------------------------------------------------------------+
|                             Great-Bevy                                |
|    (ECS 调度中心、层次化 Transform、Scene 序列化、Input/Window 系统)   |
+-----------------------------------------------------------------------+
        |                                       |
        v                                       v
+-----------------------------------+   +-------------------------------+
|     Bevy Render Graph & PBR       |   |      bevy_vendor_plugins      |
|  - 3D PBR 天体材质与光照系统      |   |  - DLSS / FSR / XeSS 契约层    |
|  - 2D 高性能几何与粒子流动力学    |   |  - Reflex / Latency 链         |
|  - Solari 光线追踪 / 路径追踪     |   |  - Ray Reconstruction 降噪     |
+-----------------------------------+   +-------------------------------+
        |                                       |
        +-------------------+-------------------+
                            |
                            v
+-----------------------------------------------------------------------+
|                    diligent-rs / diligent-sys                         |
|  (Type-Safe Rust Wrappers & Raw C FFI Bindings for DiligentCore)       |
+-----------------------------------------------------------------------+
        |                                       |
        v                                       v
+-----------------------+               +-------------------------------+
|    Direct3D 12 (DX12U)|               |      Vulkan 1.3 / Cross       |
|  - DXR 1.1 Acceleration               |  - SPIR-V / Cross-Platform    |
|  - Mesh Shaders / VRS                 |  - Mobile / Linux / Embedded  |
+-----------------------+               +-------------------------------+
```

---

## 📦 核心 Crates 清单

| Crate | 描述与定位 |
|-------|------------|
| [`bevy_ecs`](crates/bevy_ecs) | 高性能类型安全 Archetype ECS 实体组件系统，支持高并发多线程调度 |
| [`bevy_render`](crates/bevy_render) | 模块化可扩展渲染管线图（Render Graph）与着色器抽象 |
| [`bevy_pbr`](crates/bevy_pbr) | 基于物理的渲染（PBR）材质、集群光照（Clustered Forward）与阴影管线 |
| [`bevy_solari`](crates/bevy_solari) | Solari 实时光线追踪、SDF 全局光照与路径追踪子系统 |
| [`bevy_vendor_plugins`](crates/bevy_vendor_plugins) | 厂商图形 SDK 契约接口层（超分辨率/帧生成/光追降噪/超低延迟） |
| [`diligent-rs`](crates/diligent-rs) | DiligentEngine 跨平台现代图形 API 的强类型 Rust 安全绑定层 |
| [`diligent-sys`](crates/diligent-sys) | DiligentCore 原生 C API 的 bindgen FFI 绑定与 CMake 原生构建系统 |
| [`bevy_transform`](crates/bevy_transform) | 层次化空间变换与 SIMD 加速数学计算 |
| [`bevy_app`](crates/bevy_app) | 应用程序生命周期、插件注册中心与运行时调度器 |

---

## 🚀 快速启动与运行

### 1. 环境准备 (Windows MSVC)
- **Rust Toolchain**: 1.85.0 或更新版本（推荐使用 stable-x86_64-pc-windows-msvc）
- **CMake**: >= 3.20（已加入系统 PATH）
- **Ninja**: （推荐用于并行极速编译，已加入 PATH）
- **C/C++ 编译器**: Visual Studio 2022 (MSVC v143) 带 C++ 桌面开发工作负载
- **Windows SDK**: 10.0.19041 或更新版本（包含 `d3d12.h`）
- **LLVM / Clang**: 包含 `libclang.dll`（用于 bindgen 生成 FFI 绑定）

### 2. 编译与运行演示 Showcase

#### 🌌 运行 3D DXR 光线追踪与 DX12 Ultimate 演示
```powershell
cargo run --example test_demo_3d
```
> **演示内容**：
> - 3D 环绕相机视口控制与空间星体 PBR 材质（高金属度反射球体、土星环带、自发光恒星、冰晶彗星）。
> - 模拟 DXR 1.1 TLAS/BLAS 加速结构光线求交与多阶段（Primary / Shadow / Reflection / AO）光追仿真。
> - 3D 绚丽空间尾迹粒子流。
> - 实时 DirectX 12 Ultimate 4 大支柱 HUD 遥测信息看板（按 `H` 键隐藏/显示，按 `Space` 键暂停/恢复旋转，按 `R` 键重新探测硬件）。

#### 🪐 运行 2D 图形与引力粒子动力学演示
```powershell
cargo run --example test_demo_2d
```
> **演示内容**：
> - 2D 多层几何图元渲染（中心发光核、公转多轨道多行星环带、几何卫星）。
> - 动态引力轨迹微扰与绚丽发光粒子流发射系统。
> - 屏幕实时帧率、帧耗时与系统运行状态 HUD 看板。

#### 🧪 运行引擎核心子系统验证演示（Headless）
```powershell
cargo run --example test_demo
```
> **演示内容**：
> - 验证 ECS 调度器、天体引力轨道力学数值求解、多线程系统诊断与自动化生命周期管理。

---

## 🎮 演示交互快捷键

| 快捷键 | 功能说明 | 支持演示 |
|--------|----------|----------|
| `Space` | 暂停 / 恢复天体公转与自转动画 | 3D Demo |
| `H` | 切换 HUD 遥测监控看板显示 / 隐藏 | 2D / 3D Demo |
| `R` | 重新探测当前硬件 DirectX 12 Ultimate 特性支持度 | 3D Demo |
| `P` | 动态倍增粒子流发射速率（1x -> 2x -> 4x -> 1x） | 3D Demo |
| `Esc` | 优雅退出当前演示程序 | 全部 Demo |

---

## 📚 工程文档导览

项目提供了完备的架构解析与开发者指南，请参阅 [`docs/`](docs/) 目录：

- 📖 [**系统架构与设计文档** (`docs/ARCHITECTURE.md`)](docs/ARCHITECTURE.md)：深入解析 ECS 调度、Render Graph 渲染管线、DiligentEngine 接入层与厂商插件契约架构。
- 🛠️ [**全平台编译与配置指南** (`docs/BUILD_GUIDE.md`)](docs/BUILD_GUIDE.md)：详细步骤、依赖安装、CMake/Ninja 参数与常见问题排查（Troubleshooting）。
- 💡 [**特性白皮书与技术矩阵** (`docs/FEATURES.md`)](docs/FEATURES.md)：DirectX 12 Ultimate 4 大核心支柱、Solari 光追与 AI 超分辨率技术细节。
- 🎯 [**演示 Showcase 运行指南** (`docs/DEMOS.md`)](docs/DEMOS.md)：2D / 3D 演示功能、参数调节与交互控制手册。
- 🤝 [**贡献与代码规范指南** (`docs/CONTRIBUTING.md`)](docs/CONTRIBUTING.md)：代码风格规范、格式化校验（rustfmt/clippy）与提交规范。

---

## 📜 开源协议

本项目遵循以下双重开源协议（由用户自行选择）：

* [Apache License, Version 2.0](LICENSE-APACHE)
* [MIT License](LICENSE-MIT)

第三方组件遵循各自独立的开源许可协议，详见 `third_party/` 目录中的许可文件。
