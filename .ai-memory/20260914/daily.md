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
