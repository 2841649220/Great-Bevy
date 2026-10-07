# Great-Bevy 审查与修复档案（Audit）

本目录汇总多轮「健康体检 / 缺陷修复 / 规范化」的工作记录，是引擎质量状态的**唯一权威入口**。
根目录不再散落临时报告；历史原始报告以归档形式保存在 [`archive/`](archive/)。

---

## 1. 当前状态

| 维度 | 状态 | 证据 |
|---|---|---|
| 工作区编译 | ✅ 通过 | `cargo check -j 4 --workspace --all-targets` → exit 0，0 error |
| 编译告警 | ✅ 归零 | 全工作区 **0 条真实告警**（仅剩 `build.rs` 信息性 `cargo:warning`） |
| 核心 crate 测试 | ✅ 通过 | `cargo test --no-fail-fast -p bevy_ecs -p bevy_tasks -p bevy_transform` → 11 个目标，**1410 passed / 0 failed / 5 ignored** |
| 开发态运行效率 | ✅ 已修复 | `Cargo.toml` 补 `[profile.dev]` / `[profile.dev.package."*"]`；构建日志由 `[unoptimized + debuginfo]` 变为 `[optimized + debuginfo]` |
| 格式与换行 | ⚠️ 本轮改动全洁，存量 5 文件未洁 | 本轮改动的全部文件 `cargo fmt --check` 通过；`git ls-files --eol` 全仓 `w/lf`（含本轮新写入文件），无换行churn。存量 5 个**本轮未触碰**的文件不洁，见 §4.7 |
| 注释语言 | ✅ 统一英文 | `crates/` 内仅剩上游日文按键名与 `bevy_reflect` Unicode 测试标识符 |
| Clippy 告警 | ⚠️ 未清零 | 272 条（`bevy_render` 的 Diligent 迁移代码为主），清单见第 4 节 |
| 全量测试 | ⚠️ 未跑全 | 示例二进制链接 Diligent 静态库受本机 16 GB 内存限制 |

---

## 2. 里程碑

| # | 名称 | 范围 | 状态 |
|---|---|---|---|
| M1 | 核心 ECS 与并发健全性 | `bevy_ecs` / `bevy_tasks` / `bevy_window` | 完成 |
| M2 | 子系统逻辑、计时与层级加固 | `bevy_time` / `bevy_transform` / `bevy_reflect` / `bevy_platform` | 完成 |
| M3 | 渲染子系统 unsafe 审计与编译阻塞修复 | `bevy_render` + examples | 完成 |
| M4 | 代码质量、lint 与格式化统一 | 全仓 | 完成（clippy 告警见 §4） |
| M5 | 全工作区验证 | 全仓 | 完成 |
| M6 | 逐模块审查、告警归零与开发态运行效率治理 | fork 自有代码 + 构建配置 | 完成（见 §3.4、§4） |

---

## 3. 已修复问题汇总（按主题）

### 3.1 正确性与健全性（unsafe / UB / 竞态）

| 主题 | 位置 | 修复 |
|---|---|---|
| 稀疏集索引位反转与 `from_bits(0)` panic | `bevy_ecs::entity` | 改用 `from_raw_u32` + 检查式 `u32::try_from`（越界不再静默截断为 0 号实体） |
| scoped 任务 panic 时的借用释放 | `bevy_tasks::task_pool` | 保留上游 `Scope::drop` 取消语义（**曾一度被改为「等待任务跑完」，会把 panic 变卡死，已回滚**）+ 回归测试 |
| `edge_executor` 队列溢出丢任务 | `bevy_tasks::edge_executor` | `ArrayQueue` 满时溢出到 `SegQueue`，消费侧先主队列后溢出队列 |
| 热补丁资源读取数据竞争 | `bevy_ecs::schedule::executor::multi_threaded` | `HotPatchChanges` 的 tick 在 `Environment::new`（持有 `&mut World`）读取并存入原子量，改为 `Acquire` 载入 |
| 窗口句柄线程安全封装泄漏 | `bevy_window::raw_handle` | 句柄访问补充线程约束文档；`set_display_handle` 标记为 `unsafe` |
| `CommandQueue::append` 游标失配 | `bevy_ecs::world::command_queue` | 追加后重置 `other.cursor`，避免后续 `apply` 越界 `set_len` |
| 观察者触发 ID 回绕跳过新观察者 | `bevy_ecs::observer` | 「0 保留为哨兵」下沉到 `increment_trigger_id`（`u32::MAX → 1`），派发热路径不再含 `unsafe` |
| 反射远端类型 transmute 布局 | `bevy_reflect::type_registry` | 注册期校验 `size/align`，文档化 `repr(transparent)` 不变量 |
| Diligent 空指针解引用 / `mem::zeroed` 未校验 | `bevy_render::renderer::diligent_*` | 全部补 null 检查与 `SAFETY` 说明；偏移断言改为编译期常量断言 |
| `NonOwning<T>` 缺生命周期 → 安全代码可触发 UB | `diligent-rs::handle` | 补 `'a` + `PhantomData<&'a T>`，手工实现 `Clone`/`Copy`（原 `derive` 强加多余 `T: Copy`）；同步 `context.rs`/`swapchain.rs`/`texture.rs` 签名。此前文档声称“tied to the owning object”，但 `as_ref()` 是安全 fn 且类型无生命周期约束 |
| 锁中毒使渲染器永久失效 | `bevy_render::renderer::diligent_registry` | registry 4 把锁与 `CONTEXT_LOCK` 改为 poison-tolerant（`unwrap_or_else(PoisonError::into_inner)`）。diligent-rs 有 145 处 `expect()`，一次 panic 毒化即导致后续每帧 panic |

### 3.2 逻辑缺陷

| 主题 | 位置 | 修复 |
|---|---|---|
| 串行（默认特性）变换传播栈溢出 | `bevy_transform::systems` | 递归改写为显式工作栈（`Frame::Enter/Exit`），深度 1000 两种特性配置均通过 |
| 环检测预扫描 O(n²) | 同上 | 改为 `BTreeSet` 工作集，每条祖先链只走一次；遍历侧增加祖先集环保护 |
| 延迟命令执行顺序不确定 | `bevy_time::delayed_commands` | 提交顺序 `sequence`（单调计数）+ 按 `(submit_at, sequence)` 排序；提交循环先按 delay 排序 |
| `Timer::remaining`/`almost_finish` 下溢 panic | `bevy_time::timer` | `saturating_sub`（语义：elapsed 超出 duration 时剩余时间为 0） |
| 忽略歧义打印在未注册组件上 panic | `bevy_ecs::schedule` | 未知组件打印占位符而非 `unwrap` |
| 层级环导致静默跳过 / 迭代器死循环 | `bevy_transform` | 每帧环预扫描 + 警告；遍历侧二次防护（不再依赖无效的深度上限） |
| `#![expect(` 残留导致根 crate 编译失败 | `src/lib.rs` | 上一轮删除 `clippy::doc_markdown` 忽略项时留下未闭合属性，已闭合 |

### 3.3 规范与可维护性

| 主题 | 修复 |
|---|---|
| 中文代号注释（`方案 A/C`、`施工方案 §x`、`自研` 等） | 8 个文件 17 处改写为等义英文，保留内部方案编号 |
| 格式不合规包 | `bevy_anti_alias` / `bevy_solari` / `bevy_vendor_plugins` 经 `cargo fmt` 修正 |
| CRLF / 混合换行 / 制表符 / 末行换行 | 全仓归零 |
| 无谓的 API 面扩大 | `EntityIndex::from_bits` 由 `pub` 收回 crate 私有 |
| 上一轮重构残留的孤儿 lint 属性 | `bevy_transform::systems` 中 `#[expect(unsafe_code)]` 仍挂在已无 unsafe 代码的 `if` 上（递归改显式工作栈后遗留）→ 触发 `unfulfilled_lint_expectations`，已移除 |
| 嵌套 `fn` 后多余分号 / 冗余路径限定 / 未使用 glob 导入 | `bevy_ecs::observer::runner`、`bevy_tasks::task_pool`、`bevy_transform::systems` 各 1 处，已修正 |
| bindgen 重声明 C 运行时符号告警 ×5 | `diligent-sys`：`suspicious_runtime_symbol_definitions`（**非** `clashing_extern_declarations`），Windows x64 下 `-> u64` 与 `-> usize` ABI 等价，模块级 `#[allow]` |

---

### 3.4 开发态运行效率（M6）

| 主题 | 位置 | 修复 |
|---|---|---|
| **dev 全链路 `opt-level = 0`** | `Cargo.toml` | 新增 `[profile.dev] opt-level = 1` 与 `[profile.dev.package."*"] opt-level = 3`。此前 dev 构建下 ECS 查询迭代器、变换传播与整个渲染后端（均在依赖中）全部未优化，引擎实测比 release 慢一个数量级——这是“运行不高效”的根因 |
| 未启用仓库自带的快速构建配置 | `.cargo/config.toml`（新建，gitignored） | 启用 `config_fast_builds.toml` 的 Windows LLD 链接器；已用 `rustc -C linker=rust-lld.exe` 冒烟验证 |
| 逐帧读环境变量 | `bevy_render::renderer::diligent_draw` | `std::env::var_os("DILIGENT_RS_NO_RESOLVE")` 原在 `begin_tracked_render_pass`（每帧多次）内共 2 处 → 提为 `OnceLock` 缓存（Windows 下 `var_os` 每次取进程环境锁并分配） |
| registry 读路径被 `Mutex` 串行化 | `bevy_render::renderer::diligent_registry` | 4 个 `Mutex<HashMap>` → `RwLock<HashMap>`：`resolve_*` 位于逐 draw 热路径（`set_vertex_buffer`/`set_index_buffer`/间接绘制），读远多于写 |

---

## 4. 已知限制（未完成项）

1. **Clippy 告警 272 条**（clippy 1.98，集中在 `bevy_render` 的 Diligent 迁移代码）：
   - `doc_markdown` 100 条（文档反引号）、`std_instead_of_core` 35 条（`core::io` 未稳定，**有意保留**）、
   - `arc_with_non_send_sync` 24 条、`result_large_err` 12 条、`is_multiple_of` 6 条，以及示例/工具类零散告警。
2. **`cargo test --workspace` 未跑全**：示例二进制链接 ~70 MB Diligent 静态库（LTCG）受本机内存/磁盘限制；库与核心测试目标全部通过。
3. **`ResourceRegistry` 无移除路径（慢速内存增长 + 悬垂存量）**：`bevy_render::renderer::diligent_registry` 是进程级 `OnceLock`，只有 `register_*`/`resolve_*`，没有注销。
   而 `define_atomic_id!` 用 `fetch_add` 单调发号、**id 永不复用**，因此模块文档中“re-registration overwrites stale entries / 只有 id 复用才会命中陈旧项”的论证与事实不符。
   后果：① 条目随累计创建无限增长（存活期与进程同长）；② 已析构资源的裸指针永久留存。因 id 不复用，当前**不构成 UAF**，属慢速泄漏。
   建议修复（未做，需先建验证路径）：存储侧改存 `Weak`，`resolve_*` 时 `upgrade()` —— 既能在失败时惰性剔除陈旧项，又能让 resolve 返回强引用从而彻底消除悬垂隐患。
4. **`Cargo.lock` 未纳入版本控制**：`.gitignore` 忽略 `Cargo.lock`（继承自上游“库”定位），但本仓库交付引擎与示例，构建不可复现、依赖会漂移。建议 `git add -f Cargo.lock` 并移除该忽略规则（属仓库策略变更，待确认）。
5. **LLD 链接 Diligent 静态库未验证**：`third_party/diligent-build` 中的 `DiligentCore.lib`（565 MB）以 MSVC LTCG（`/GL`）构建，LLD 对 LTCG 目标文件支持有限。本轮只验证了不链接 Diligent 的目标（`cargo check` 全工作区、三者核心 crate 测试全部通过并成功链接）。若链接 Diligent 的目标失败，删除 `.cargo/config.toml` 即可回退（代价是链接变慢，不影响正确性）。
7. **存量 `cargo fmt --check` 不洁（5 文件，本轮未触碰）**：`bevy_render::render_resource::bind_group`、`bevy_render::render_resource::texture`、`bevy_render::renderer::diligent_draw:652`、`diligent-rs/build.rs`、`diligent-rs/examples/triangle.rs`、`diligent-sys/build.rs`。这些文件本轮均未修改，diff 形态（match 臂尾逗号、链式调用换行、fn 签名单行化）符合 rustfmt 版本漂移特征（`rustfmt.toml` 固定 `style_edition = "2021"`，当前 rustfmt 为 1.9.0-stable / 工具链 1.98.1）。**未执行 `cargo fmt`**：会引入与本轮无关的大面积 diff，且可能与 CI 期望的 rustfmt 版本冲突，需先确认应当以哪个 rustfmt 版本为准。据此，§1 中“全部包 fmt 通过”的原结论应修正为上述表述。
8. **`CONTEXT_LOCK` 全局串行化**：`diligent_registry::context_guard()` 为每一次 Diligent 立即上下文调用加全局 `Mutex`。diligent-rs 明确记录立即上下文**非线程安全**（引擎录制到单条 D3D12 命令列表），故当前串行化是**正确性所需**；但渲染世界调度是多线程的，这是渲染录制的结构性瓶颈。彻底消除需让渲染图在单线程上录制，属架构级改动，未做。

---

## 5. 事故记录（2026-09-14）

在批量清理行尾空白时误用大小写不敏感的 `-replace`，误删行尾 `t`/`T`/反引号，波及 1100+ 文件。处置：git 检查点 → strip 匹配 / 词表校验 / 前缀匹配三轮定向还原 → 351 个未被前序改动污染的文件从 HEAD 全量还原 → 依据编译器逐项修复。最终工作区收敛为 87 个文件（2368+/978-），编译与核心测试全绿。

> 教训：批量文本改写必须使用大小写敏感、字符集显式且带干跑（`-WhatIf`）的实现；破坏性操作前先落检查点。

---

## 6. 归档

| 文件 | 说明 |
|---|---|
| [`archive/round1-project.md`](archive/round1-project.md) | 首轮：健康体检问题清单与里程碑（含逐条证据） |
| [`archive/round2-debug.md`](archive/round2-debug.md) | 次轮：独立复现的 12 项缺陷与修复验证 |
| [`archive/round3-normalization.md`](archive/round3-normalization.md) | 本轮：全仓规范化、审查发现与事故披露 |

项目记忆（逐日操作日志）位于 [`.ai-memory/`](../../.ai-memory/)。
