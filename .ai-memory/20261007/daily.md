# 2026-10-07

## [22:08] - M7 轮验证、编译修复、推送与文档回填

- **入口状态**：工作区有 26 个未提交文件（+715/−194），且 `target/` 已被上一日的 `cargo clean` 清空 → 这批改动**没有任何编译证据**。`.ai-memory/20260914/daily.md` 只记到 15:40 / 16:20 两条（自述「14 文件 / +189 −51」），说明工作区内容是该记录之后的**未记录轮次**，且与 `docs/audit/README.md` §4.3「registry 无移除路径，本轮未做」直接矛盾（代码已经实现了移除路径）。
- **该轮（记为 M7）改动主题**（细节见 `docs/audit/README.md` §3.5）：registry 注销 token（`RegistryRegistration`）、每线程可重入 `CONTEXT_LOCK` 并在整 pass 期间持锁、阻塞映射前先建 fence 等待 GPU、`clear_buffer` 由桩改为分块 `UpdateBuffer`、renderer 初始化的 backend/device/feature 校验与 `RenderQueue::attach` 位置调整、feature mask 收回未实现能力、`NonOwning<'a, T>` 与 `MappedBuffer`/`MappedTexture` 的借用、`SwapChain::resize` 转 `unsafe`、串行 `mark_dirty_trees` 的环保护、dev profile `opt-level`。
- **验证与修复**：
  - 首轮 `cargo check -j 4 --workspace --all-targets` → **E0599 失败**：`bevy_render/src/render_resource/buffer.rs:272` 对 `&diligent_rs::RenderDevice` 调用了只存在于 bevy_render `RenderDevice` 上的 `wait_for_gpu`。
  - 修复 3 处：`renderer/render_device.rs` 把 fence 逻辑抽成以原生句柄为参数的自由函数 `wait_for_gpu(device, context)`（原方法改为委托）；`buffer.rs` 改调该自由函数；`settings.rs:241` 用已导入的 `Backends::DX12` 替换 `wgpu_types::Backends::DX12` 以消除 `unused-qualifications`（首版写成全限定路径本身又触发一次该 lint，改为非限定调用后归零）。
  - 复跑 `cargo check -j 4 --workspace --all-targets` → **exit 0**，2m59s（失败的那一轮是冷缓存，未计时）。`cargo check -p bevy_render --all-targets` → exit 0，**bevy_render 自身 0 告警**。推送后又对同一代码状态复跑全工作区 → exit 0，1m03s。
  - 逐包 `cargo fmt -p <pkg> -- --check`（`bevy_render` / `bevy_ecs` / `bevy_tasks` / `bevy_transform` / `diligent-rs` / `diligent-sys`）：本轮触碰文件全部干净；不洁仅 `bevy_render/src/render_resource/bind_group.rs:90` 与 `bevy_render/src/renderer/diligent_mapping.rs:766,778`，两文件本轮均未改。**注意**：`cargo fmt --all --check` 在本机无法运行（`文件名或扩展名太长。 (os error 206)`，rustfmt 拼接的命令行超 Windows 上限），所以存量清单只能按包统计。
  - 未复跑测试：M7 只改 `bevy_render` + `diligent-rs`，而 `bevy_render` 的测试目标需要链接 565 MB `DiligentCore.lib`（本机 16 GB，见 §4.2）；`bevy_ecs`/`bevy_tasks`/`bevy_transform` 本轮未被再次触及，沿用 M6 的 1410 passed / 0 failed。
- **提交与推送**：commit `4809a9ba`（26 files，+728/−194）→ `git push origin main` 成功（`3cee649e..4809a9ba`，一并送出上一会话遗留的 `d4803dbf`）。推送后 `git status` 干净、与远程同步。
- **工具链漂移（新发现，影响既有结论可复现性）**：rustc 1.98.1 → **1.99.0**（2026-09-28），rustfmt 1.9.0-stable → **1.10.0-stable**。全工作区因此多出 10 处 `deprecated` 告警（`std::f32::{INFINITY,MAX,NEG_INFINITY,EPSILON}` 改名、`Atomic::fetch_update` → `try_update`），分布于 `bevy_ui/src/ui_node.rs`(5)、`bevy_ecs/src/world/identifier.rs:36`、`bevy_picking/src/window.rs:46`、`bevy_math/src/primitives/half_space.rs:149`、`bevy_core_pipeline/src/core_3d/mod.rs:487`、`bevy_sprite_render/src/sprite_mesh/sprite_material.rs:272`，**均不在本轮触碰文件内**。上一轮「全工作区 0 条真实告警」的结论在当前工具链下不成立，已在 §1/§4.6 修正为该口径。
- **文档回填**：`docs/audit/README.md` 增 M7 里程碑行 + 新增 §3.5；§1 改写「工作区编译 / 编译告警 / 核心 crate 测试 / 格式与换行」四行为可复现口径；§4 连续重编号（原 1,2,3,4,5,7,8 缺 6）——§4.3 从「无移除路径 + 建议 Weak/upgrade」改为「已注销 token，但 `resolve_*` 仍返回裸指针、安全性依赖使用方借用」，§4.6 更新 fmt 清单与 `--all` 不可用说明，§4.7 补充「M7 只做线程内重入，未改变跨线程互斥」，新增 §4.8「M7 无 GPU 运行期证据」。
- **换行安全**：本轮全部为定点 `edit` + 新建单日文件，无批量文本改写；`git ls-files --eol` 对本轮改动文件均为 `w/lf`，与 `.gitattributes`（`*.rs/*.toml text eol=lf`）一致。
- **遗留 / 待用户决策**：`Cargo.lock` 是否 `git add -f` 入库（§4.4，仓库策略变更）；clippy 272 条（§4.1）；LLD 链接 Diligent 静态库（§4.5）；`CONTEXT_LOCK` 跨线程串行化的结构性瓶颈需渲染图单线程录制（§4.7）。
