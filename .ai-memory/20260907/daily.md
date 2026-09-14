# 2026-09-07

## 清理构建缓存
- 工作区 C:\Users\ASUS\Desktop\Bevy（Bevy 引擎 Rust 仓库）磁盘告急：C 盘仅剩 3.4G（99%）。
- 执行 `cargo clean` 清理 `target/`，移除 74339 个文件、74.5 GiB（debug 71G + release 351M）。
- 清理后 C 盘剩余 75G（77%）。源码与 Cargo.lock 未受影响。
- 工作区内无 .tmp/.bak/.log 等临时文件残留。
- 未清理项（待用户确认，位于个人目录）：`~/.cargo/registry` 1.2G、`~/.cargo/git` 366M。

## 环境备注
- cargo 1.97.0 (2026-06-30) 可用。
- 下次 `cargo build` 需全量重编译 Bevy，耗时较长（数十分钟~数小时）。
