# Windows D3D12 差异白名单格式规范（whitelist/README.md）

Project: rendering-backend-replacement (Bevy 0.19 fork)
Sources: `tests/reference-frames/README.md` §2（白名单档位与方法论）、施工方案 §11.4（D3D12 档预置类别）、§13.2.5（第二应急扩展）
Status: **预置占位（Task 13）—— 全部 `expected_differences` 为空；compare 子命令接线属 M2a 对照期**

## 1. 目的

参考帧来自 **wgpu-Vulkan** 路径，替换后对照 **Diligent-D3D12** 输出。D3D12 档位为"阈值放宽 + 白名单"机制：允许 D3D12 后端系统性差异落入白名单，随机差异与采样/浮点类未查明差异一律排查（`tests/reference-frames/README.md` §2 流程 1–5）。

本目录是白名单的**文件载体**：每场景一个 JSON，运行时按实测填充 `expected_differences`。本任务只预置结构与语义，不虚构任何具体差异值。

## 2. 文件清单与命名

| 文件 | 场景 |
|---|---|
| `whitelist-d3d12-3d_scene.json` | 3d_scene |
| `whitelist-d3d12-pbr.json` | pbr |
| `whitelist-d3d12-lighting.json` | lighting |
| `whitelist-d3d12-bloom_3d.json` | bloom_3d |
| `whitelist-d3d12-ssao.json` | ssao |
| `whitelist-d3d12-lightmaps.json` | lightmaps |
| `whitelist-d3d12-irradiance_volumes.json` | irradiance_volumes |
| `whitelist-d3d12-deferred_rendering.json` | deferred_rendering |
| `whitelist-d3d12-meshlet.json` | meshlet |

命名规则：`whitelist-<candidate-backend>-<scene>.json`（当前档位 = D3D12）。JSON **禁止内嵌注释**（严格 JSON）；语义一律用约定字段（`notes`）承载。

## 3. Schema 规范（v1.0）

### 3.1 顶层字段

| 字段 | 类型 | 语义 |
|---|---|---|
| `schema_version` | string | 本规范版本，当前 `"1.0"`；M2a 如需扩展（如按帧/按通道档位）必须升版并同步本文档 |
| `whitelist_id` | string | 唯一标识 `whitelist-d3d12-<scene>`，与文件名一致 |
| `scene` | string | 场景 id，须与场景注册表 `scenes.rs` 的 `id` 一致 |
| `tier` | string | 档位标识，当前 `"windows-d3d12-whitelist"`（README §2 预置档） |
| `platform_pair` | object | 对照对：`baseline` = wgpu / vulkan（参考来源），`candidate` = diligent / d3d12（替换对象） |
| `thresholds` | object | **D3D12 档放宽阈值**（§3.2），数值 M2a 实测校准后定稿 |
| `categories` | array | 四类预置类别（§3.3、§4），每类含 `expected_differences` 与 `notes` |
| `notes` | string | 场景级语义说明（采集状态、预期相关类别、约束），禁止在此写具体差异值 |

### 3.2 `thresholds`（档位放宽阈值，占位）

键名与 `tools/reference_frames/src/metrics.rs` 的 `DiffStats` 字段对齐，便于 M2a 接线直接映射：

| 键 | 类型 | 语义 |
|---|---|---|
| `ssim` | number\|null | SSIM 下限（0..1），null = 未定 |
| `psnr_db` | number\|null | PSNR 下限 dB（`inf` 序列化按 `"inf"`），null = 未定 |
| `mean_abs_diff` | number\|null | 平均绝对通道差上限，null = 未定 |
| `max_abs_diff` | number\|null | 最大绝对通道差上限，null = 未定 |
| `diff_histogram_p95` | number\|null | 直方图 95 分位 bin 上限（差异像素占比上限），null = 未定 |
| `notes` | string | 放宽依据：D3D12 后端系统性差异（光栅化规则/深度范围/采样坐标/blend 精度）所致；阈值须在 M2a 实测分布后校准，不得预先编造 |

**null 语义**：未定档，M2a 对照期按实测直方图校准后以数值回填。当前全部为 null。

### 3.3 `categories[]` 条目字段

| 字段 | 类型 | 语义 |
|---|---|---|
| `id` | string | 稳定 id（kebab-case）：`rasterization_rules` / `depth_range` / `sample_coordinates` / `blend_precision` |
| `label` | string | 中文名（光栅化规则 / 深度范围 / 采样坐标 / blend 精度） |
| `description` | string | 该类覆盖的差异范围 |
| `policy` | string | `"whitelistable"`：系统性差异可入档（须有理由）；`"investigate_first"`：必须先排查后入档（README §2 流程 4、§10.3 原则） |
| `expected_differences` | array | 实测差异条目，**当前为空**（§3.4 条目规格），M2a 按实测填充 |
| `notes` | string | 该类预置语义：允许什么、禁止什么、M2a 怎么填 |

### 3.4 `expected_differences[]` 条目规格（M2a 填充时使用）

```json
{
  "frame": "0210",
  "metric": "ssim | psnr_db | mean_abs_diff | max_abs_diff | diff_histogram_p95",
  "observed": 0.9823,
  "threshold": 0.98,
  "scope": "whole_frame | { \"region\": [x0, y0, x1, y1] }",
  "rationale": "系统性差异理由（后端特性/规格依据），须可复核"
}
```

- `metric` 键名与 `DiffStats` / 顶层 `thresholds` 对齐；`psnr_db` 为 `inf` 时 `observed` 写字符串 `"inf"`。
- `scope` 默认 `whole_frame`；区域条目给出像素矩形。
- 每个条目必须 `observed` 与 `threshold` 成对且 `rationale` 非空；**不得无理由入档**。
- 采样/浮点类差异（`sample_coordinates` 策略类）在完成排查并有结论前**禁止**写入条目。

## 4. 类别定义（§11.4 预置四类）

| id | label | 定义 | policy |
|---|---|---|---|
| `rasterization_rules` | 光栅化规则 | D3D12 与 Vulkan 光栅化规则差异所致的系统性边界差异：top-left fill rule 与像素中心约定、三角边沿裁剪规则、光栅化覆盖测试差异（MSAA 已强制 Off，仍存在覆盖/边沿规则差异） | `whitelistable` |
| `depth_range` | 深度范围 | D3D12 深度约定差异：clip 空间深度 [0,1]（near=0）与 Bevy reverse-Z（1.0=近、0.0=远，`tests/reference-frames/README.md` §5.1）之间的系统性变换/偏置差异；深度 clear 值、深度比较映射差异 | `whitelistable` |
| `sample_coordinates` | 采样坐标 | 纹素中心/半纹素偏移约定、梯度（ddx/ddy）与 mip 选择差异所致的系统性采样差异 | `investigate_first` |
| `blend_precision` | blend 精度 | 固定功能混合在 D3D12 与 Vulkan 间的舍入/精度系统性差异（HDR/浮点混合、blend 边沿通道小偏移） | `whitelistable` |

**入档资格判定（README §2 流程 2–4）**：系统性差异（后端特性所致、逐帧稳定）→ 可入档并写理由；随机差异（逐帧抖动/不一致）→ 一律排查，禁止入档；采样/浮点类差异 → 一律先排查（§10.3），查明系统性后才可入档。

## 5. 阈值放宽容许度（Relaxation Allowance）

- 本档位为 **D3D12 放宽档**：相对 Linux 同后端主参照（SSIM ≥ 0.99）阈值允许放宽，但**放宽只限预置四类的系统性差异来源**。
- 放宽容许度边界：不得以放宽为名掩盖随机差异、状态管理缺陷或未排查的采样/浮点类差异。
- 阈值定稿要求：M2a 先跑 `compare` 得到逐帧 SSIM/PSNR/直方图分布（README §2 流程 1），再按分布取档；数值须留注释依据（放进条目 `rationale` 或类别 `notes`）。
- 超出白名单 → 第一应急：排查差异来源，修复至白名单内；第二应急：扩展白名单（仅限系统性差异，必须文档化理由，§13.2.5）。

## 6. 填充流程（M2a 对照期）

1. **实测**：Diligent-D3D12 输出与 wgpu-Vulkan 参考帧逐帧跑 `cargo run -p reference_frames -- compare <ref> <candidate>`（`diff_histogram` 直方图辅助判系统性/随机）。
2. **分类**：按 §4 判定差异归属类别与资格（系统性→候选入档；随机/未查明→排查，不入档）。
3. **评审**：每条 `expected_differences` 条目带 `observed`/`threshold`/`rationale`，理由可复核；采样/浮点类须附排查结论。
4. **入档**：回填对应 JSON 的 `expected_differences`，同步在 `tests/reference-frames/README.md` §8 记录理由与范围（该文件已于 Task 12（2026-08-05）更新采集记录，M2a 对照期继续在 §7/§8 记录）。

## 7. compare 子命令接线现状

**已接线（2026-08-08，M2a 任务 1.4）**。`tools/reference_frames/src/whitelist.rs` 实现了 whitelist 加载与判定（`Whitelist`/`Thresholds`/`Category`/`ExpectedDifference` schema 类型 + `judge()` 判定器），`main.rs` 的 `compare` 子命令已支持：

```powershell
cargo run -p reference_frames -- compare <reference> <candidate> --whitelist tests/reference-frames/whitelist/whitelist-d3d12-<scene>.json --frame <nnnn>
```

- 输出 `DiffStats` JSON 后追加 `WhitelistVerdict` JSON（`checks[]` 每指标 `{metric, observed, threshold, passed}` + 整体 `passed`）。
- 判定规则：`ssim`/`psnr_db` 越高越好，其余指标越低越好；`thresholds` 中 `null` 值跳过（未校准）；每帧 `expected_differences` 条目（`frame` 匹配 `--frame`）覆盖全局阈值；任一指标不通过则整体 `passed=false` 且进程退出码 9。
- 现有 9 个 whitelist JSON 的 `thresholds` 仍为 `null`（M2a 对照期实测后回填），`--whitelist` 判定此时 `checks=[]`、`passed=true`（空判定）。
- 配套单测：`whitelist::tests`（5 例：同图通过、低 SSIM 失败、每帧覆盖、直方图 p95、`"inf"` 解析）。

**接线说明**：schema 依据本规范（§3），字段名与 `metrics.rs` `DiffStats` 对齐；`WhitelistVerdict` 字段名为 `whitelist_id`/`scene`/`checks`/`passed`（与 `expected_differences` 条目 `metric` 键名对齐）。
