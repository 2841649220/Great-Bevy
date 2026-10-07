# 2026-09-14

## [02:20] - 全面规范化 / 审查 / Debug（Bevy 仓库本轮）

- **范围**: 全仓 Rust 源码（1838 个 `.rs` / 56 万行）+ 上一轮未提交改动（86 个文件）复核。
- **审查并修复**:
  1. `bevy_tasks::scope` panic 路径：上一轮用 `ScopeDropGuard` 把“取消任务”改成“等待任务跑完”，会把 panic 变成卡死；恢复上游取消语义（`Drop for Scope` + `cancel().await`）并新增回归测试。
  2. `bevy_ecs::observer` 触发 ID 回绕：把“跳过 0 哨兵”下沉到 `UnsafeWorldCell::increment_trigger_id`，派发热路径去掉 unsafe 与分支，SAFETY 文档改为真实不变量。
  3. `EntityIndex/Entity::get_sparse_set_index`：`value as u32` 静默截断 → `u32::try_from` 检查式转换。
  4. `EntityIndex::from_bits`：为测试扩大的 `pub` 收回为 crate 私有。
  5. `src/lib.rs`：上一轮删除 `#![expect(clippy::doc_markdown)]` 时残留 `#![expect(`，导致 crate 级括号未闭合（编译失败）→ 闭合并移除。
  6. 注释规范化：8 个文件 17 处中文代号注释改写为英文；CRLF/混合换行/制表符/末行换行全部归零。
  7. `cargo fmt -p` 修正 3 个不合规包（bevy_anti_alias / bevy_solari / bevy_vendor_plugins）。
- **事故与恢复**: 清理行尾空白时误用大小写不敏感 `-replace`，误删行尾 `t/T`/反引号，波及 1100+ 文件；通过 git 检查点 + strip/词表/前缀三轮定向还原 + 351 个未改动文件从 HEAD 全量还原 + 编译器逐项修复，最终收敛为 86 个文件 / 2366 插入 / 976 删除。
- **验证**:
  - `cargo check -j 4 --workspace --all-targets` → **0 error**（迭代收敛 CLEAN）
  - `cargo test -j 4 --no-fail-fast -p bevy_ecs -p bevy_tasks -p bevy_time -p bevy_transform -p bevy_platform -p bevy_window` → **exit 0，597 passed / 0 failed**
  - `git ls-files --eol` 异常 0；`git grep -P '\t'` 0 命中
- **遗留**: clippy 1.98 告警未逐条清理（`doc_markdown` 100 / `std_instead_of_core` 35（core::io 未稳定，有意保留）/ `arc_with_non_send_sync` 24 / `result_large_err` 12 等）；`cargo test --workspace` 未跑全（示例二进制链接受本机内存限制）。

## [04:10] - 文档整理、冗余清理与提交推送

- **文档整理**:
  - 三份根目录报告（PROJECT.md / DEBUG_REPORT.md / NORMALIZATION_REPORT.md）归档为 `docs/audit/archive/round{1,2,3}-*.md`；
  - 新建 `docs/audit/README.md` 作为唯一权威入口（当前状态 / 里程碑 / 已修复问题汇总 / 已知限制 / 事故记录 / 归档索引）；
  - README 文档导览新增审计档案入口；修正过时事实：Rust 工具链 1.85 → 1.95（README 徽章 + 快速启动 + BUILD_GUIDE），
    clippy 门禁说明由 `-- -D warnings` 改为与当前告警现状一致（存量告警清单指向 docs/audit/README.md §4）。
- **清理**:
  - 删除 `.agents/`（89 个多智能体临时文件）、`.workbuddy/`（记忆迁至 `.ai-memory/20260907/daily.md`）；
  - `.gitignore` 增加 agent 临时目录（`.agents/`、`.workbuddy/`、`.debug_*/`）忽略规则；
  - `assets/models/aimisi/*.glb`（4 个 / 158 MB / 全仓零引用）**未删除也未提交**，等待用户确认后处置。
- **提交与推送**: commit `98a36584`（103 files, +4287/-983）→ `git push origin main` 成功（`e724232f..98a36584`）。
  - 提交内容已核验：不含 `target/`、`third_party/` 或 `.glb` 二进制；工作区仅剩上述待确认资源。

## [04:35] - 按用户确认清理无用资源与构建缓存

- **删除** `assets/models/aimisi/*.glb`（4 个 / 158 MB / 全仓零引用）：经用户确认后，**移入 Windows 回收站**（可恢复），非永久删除。
- **清理** `target/` 构建缓存：`cargo clean` 移除 35 453 个文件 / 27.2 GiB。
- **磁盘**：C 盘可用空间 41.61 GB → 67.35 GB（释放 25.74 GB）。
- **仓库状态**：`git status` 干净；顶层不再包含 `.agents/`、`.workbuddy/`、`target/` 与临时报告。
- **注意**：构建缓存已清空，下次 `cargo build/test` 需全量重编译（数十分钟以上）；已提交并推送的代码状态不受影响。
## [15:40] - Review/Debug/性能治理（Bevy 仓库本轮 · 模块化审查 + 高效运行）

- **范围**: 全仓 1848 个 `.rs`；重点为 fork 自有代码 (~20k 行: `crates/diligent-rs` 12k / `crates/diligent-sys` 1k / `bevy_render` Diligent 迁移 4.4k + 27 个集成文件)，上游 Bevy 代码按已知缺陷史定向复核。
- **基线**: 冷缓存 `cargo check -j 4 --workspace --all-targets` → exit 0，4m54s，0 error，9 条真实告警。

- **修复（已编译验证，exit 0）**:
  1. `bevy_ecs/src/observer/runner.rs` 嵌套 fn 后多余分号 → 去除。
  2. `bevy_tasks/src/task_pool.rs` 冗余 `crate::` 限定 → 去除。
  3. `bevy_transform/src/systems.rs` 测试模块未使用 glob 导入 → 收窄为 `use bevy_ecs::world::CommandQueue`。
  4. `bevy_ecs/src/entity/mod.rs` `EntityIndex::from_bits` 仅测试可达 → 加 `#[allow(dead_code, reason=...)]`（不用 `expect`，否则 test target 会报 unfulfilled）。
  5. `diligent-sys/src/lib.rs` bindgen 重声明 C 运行时符号（strlen/mem*）触发 `suspicious_runtime_symbol_definitions` ×5 → 模块级 `#[allow]`（**注意：lint 名不是 `clashing_extern_declarations`**）。

- **效率修复（核心交付）**:
  1. `Cargo.toml` 新增 `[profile.dev] opt-level=1` + `[profile.dev.package."*"] opt-level=3`。此前 dev 全链路 `opt-level=0`，引擎依赖（ECS/渲染/数学）全部未优化 —— 这是"运行不高效"的根因。构建日志由 `[unoptimized + debuginfo]` 变为 `[optimized + debuginfo]`。
  2. 新建 `.cargo/config.toml`（gitignored，本地开发配置），启用仓库自带 `config_fast_builds.toml` 的 Windows LLD 链接器；已用 `rustc -C linker=rust-lld.exe` 冒烟验证通过。
  3. `bevy_render/src/renderer/diligent_draw.rs`：`std::env::var_os("DILIGENT_RS_NO_RESOLVE")` 原位于 `begin_tracked_render_pass`（每帧多次）内，两处 → 提为 `OnceLock` 缓存 helper `msaa_resolve_disabled()`。Windows 下 `var_os` 每次取进程环境锁并分配。
  4. `diligent_registry.rs`：4 个 `Mutex<HashMap>` → `RwLock<HashMap>`（resolve 侧在逐 draw 热路径，读远多于写）；全部 registry 锁与 `CONTEXT_LOCK` 改为 poison-tolerant（`unwrap_or_else(PoisonError::into_inner)`），避免一次 panic 永久毒化后整个渲染器再也无法使用。

- **健全性修复**: `diligent-rs/src/handle.rs` `NonOwning<T>` 补 `'a` 生命周期 + `PhantomData<&'a T>`，手工实现 `Clone`/`Copy`（原 `derive` 会强加多余 `T: Copy` 约束）。此前文档声称"tied to the owning object"但类型无生命周期，`as_ref()` 是安全 fn 却可在 owner drop 后产生 UB。已同步 `context.rs` / `swapchain.rs` / `texture.rs` 三处签名。

- **验证**:
  - `cargo check -j 4 --workspace --all-targets` → **exit 0，0 error，0 代码告警**（8m55s，新 profile 下）
  - `cargo check -p diligent-sys` → **exit 0，0 告警**（仅剩 build.rs 信息性 cargo:warning）
  - 核心 crate 测试见下一条记录。

- **已知限制 / 未验证项**:
  1. LLD 链接器对 `third_party/diligent-build` 中 LTCG（/GL）静态库的兼容性未验证 —— 链接 DiligentCore 的目标（bevy_render 测试、examples）未跑；失败时删除 `.cargo/config.toml` 即可回退。
  2. dev profile `opt-level=3` 使 `cargo check` 由 4m54s 增至 8m55s（约 1.8×），换取运行期大幅提速；属标准 Bevy 取舍。
  3. `ResourceRegistry` 无移除路径且 id 单调不复用（`define_atomic_id!` 为 `fetch_add`），进程级 `OnceLock` 导致条目随累计创建无限增长，存量为悬垂指针；因 id 不复用故当前不构成 UAF，属慢速内存增长。修复需生命周期化（存 `Weak` + resolve 时 upgrade，同时消除悬垂隐患），本轮未做。
  4. `Cargo.lock` 被 `.gitignore` 忽略且未入版本控制 → 构建不可复现，建议 `git add -f Cargo.lock` 并移除忽略规则（待用户决策）。
## [16:20] - 本轮最终验证与自查结论

- **验证证据（全部 exit 0）**:
  - `cargo check -j 4 --workspace --all-targets` → **exit 0，0 error，0 真实告警，0 unfulfilled lint expectation**
  - `cargo check -p diligent-sys` → exit 0，**0 告警**（5 条 `suspicious_runtime_symbol_definitions` 已清除）
  - `cargo check -p bevy_render --all-targets` → exit 0（覆盖 diligent-rs + diligent_draw 的最终改动）
  - `cargo test -j 4 --no-fail-fast -p bevy_ecs -p bevy_tasks -p bevy_transform` → **exit 0，11 个目标全 ok，1410 passed / 0 failed / 5 ignored**（含 doc-test）
- **自查发现并修正的自身问题**:
  1. 写入 `handle.rs` 文档时误留 10 处**字面量** `\``（反斜杠+反引号），会使 intra-doc 链接 `[`NonOwning::as_ref`]` 失效 → 已全部还原为普通反引号，并复查全部改动 `.rs` 文件确认归零。
  2. `diligent_draw.rs` 新增 helper 与其后文档注释之间缺空行 → 已补。
- **换行安全（吸取 2026-09-14 事故教训）**: 本轮全部为定点 `edit` 补丁，未做任何批量文本改写；`git diff --stat` 显示仅插入/局部替换，**无换行 churn**；`git ls-files --eol` 显示全部改动文件 `w/lf`，与 `.gitattributes` 的 `*.rs/*.toml text eol=lf` 一致。
- **新增发现（未修，已记入 docs/audit/README.md §4.7）**: 存量 5+1 个**本轮未触碰**文件不满足当前 `cargo fmt --check`（`bind_group.rs`、`render_resource/texture.rs`、`diligent_draw.rs:652`、`diligent-rs/build.rs`、`diligent-rs/examples/triangle.rs`、`diligent-sys/build.rs`）。diff 形态符合 rustfmt 版本漂移（`style_edition = "2021"`，当前 rustfmt 1.9.0-stable）。**有意未执行 `cargo fmt`**：会产生与本轮无关的大面积 diff，需先确认基准 rustfmt 版本。据此修正了 audit §1 中“全部包 fmt 通过”的原表述。
- **改动清单**: 14 文件，+189 / -51（含 `.ai-memory` 与 `docs/audit/README.md`）；`.cargo/config.toml` 为 gitignored 的本地配置，不计入。
