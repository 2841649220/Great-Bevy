# bevy_vendor_plugins — 厂商插件接口框架（M3a）

> 交付物：五类插件接口契约 + 探测协议 + 适配器模板 + TAAU 互斥模型
> 依据：spec §5.7、design §2.2.2.4、《渲染后端替换最终方案 v2.0》、《渲染后端替换工程施工方案》§7.1

## 1. 定位

本 crate 是渲染后端替换工程的**契约层**：定义厂商 SDK 适配器需要实现的 trait 面与渲染器传给它的数据形状，**不捆绑、不链接任何厂商 SDK**。无厂商 SDK 时引擎完整可用（内置 FXAA/SMAA/TAA/CAS 路径）。

## 2. 五类插件接口（design §2.2.2.4 / 施工方案 §7.1.2）

| Trait | 覆盖 | 挂接点 |
|-------|------|--------|
| `UpscalerPlugin` | DLSS SR、FSR Upscaling、XeSS-SR、ARM ASR、SGSR 1/2、星速引擎 AI 超分 | Core3dSystems 链 upscaling 集（`.after(Core3dSystems::PostProcess)`） |
| `FrameGenPlugin` | DLSS MFG/DMFG、FSR FG、XeSS FG/MFG | proxy swapchain |
| `DenoiserPlugin` | DLSS RR、FSR Ray Regeneration | RT 输出降噪 |
| `LatencyPlugin` | Reflex、Anti-Lag 2、XeLL | 帧延迟链 |
| `PureAaPlugin` | DLAA、FSR Native AA | 与内置 AA 同接口 |

**统一输入**（`UpscaleInput`）：color / depth / motion vectors / jitter 相位 / reactive mask / mip bias；**质量档位**（`UpscaleQuality`）：NativeAA/Quality/Balanced/Performance/UltraPerf。

**降噪信号集**（`DenoiseSignals`）：Direct/Indirect Diffuse/Specular、AO、Specular Occlusion、Dominant Light、linear depth、motion vectors（全部可选）。

## 3. 探测协议（施工方案 §7.1.1）

- **目录约定**：`plugins/<backend>/<vendor>/`（如 `plugins/dx12/nvidia/`、`plugins/vk/arm/`）。
- **实现**：`PluginProbe` trait + `DirectoryProbe`（目录扫描）。开发者在 vendor 目录放入 SDK 标记文件（如 `sdk-version.txt`）即视为"命中"。
- **语义**：命中注册、未命中自动屏蔽；未探测到任何 SDK 时引擎完整可用；诊断日志输出缺失原因。
- **有/无 SDK 两态**均可用（验收矩阵见 §6）。

## 4. 适配器模板（施工方案 §7.1.3）

- `UpscalerTemplate`：UpscalerPlugin 的示例实现（no-op），开发者复制后填入 SDK 调用即可启用。
- SDK 本体**不捆绑、不发行**（§7 法律与授权）；开发者自备 SDK 放入 plugins/ 目录。
- Android 插件集（M5b）：ARM ASR（通用基准）、SGSR2（Adreno 优先）、星速引擎 AI 超分（厂商 SDK 自备）。

## 5. TAAU 与独立 TAA 互斥（施工方案 §7.1.4）

- 所有 TAAU 超分器（FSR/ASR/SGSR2/DLSS SR）内置 temporal AA，启用时替代独立 TAA pass。
- `resolve_taa_exclusivity(taau_registered)`：返回 `TaauActive`（禁用独立 TAA）或 `IndependentTaa`（保留）。
- 与 Camera TAA/DLSS 二选一互斥模型一致。

## 6. 探测矩阵验证

| 状态 | 行为 | 验收 |
|------|------|------|
| 有 SDK | 插件正常注册启用 | 各适配器挂接成功 |
| 无 SDK | 引擎完整可用（内置 AA/超分路径）+ 诊断日志输出缺失原因 | 引擎启动无插件依赖错误 |

## 7. 使用

```rust
use bevy_vendor_plugins::prelude::*;

// 探测（启动时）
let probe = DirectoryProbe::new("dx12", "sdk-version.txt");
for hit in probe.probe(&std::path::Path::new("plugins")) {
    tracing::info!("SDK {}:{} available={}", hit.vendor, hit.version.unwrap_or_default(), hit.available);
}

// 适配器注册（有 SDK 时）
app.add_plugins(MyDlssAdapter); // 实现 UpscalerPlugin

// TAA 互斥
if resolve_taa_exclusivity(upscaler_registered) == TaaExclusivity::TaauActive {
    // 跳过独立 TAA pass
}
```