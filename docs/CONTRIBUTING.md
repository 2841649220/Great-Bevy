# Great-Bevy 贡献与开发规范指南

欢迎参与 **Great-Bevy** 次世代游戏引擎的开源建设！为了保证代码库的质量、一致性与长久可维护性，请遵循以下开发规范。

---

## 1. 代码风格与规范

### 1.1 Rust 版本与 Edition
- 本项目遵循 **Rust 2024 Edition** 标准。
- 保证代码在最新稳定版（Stable）Rust 下零编译告警（Zero Compiler Warnings）。

### 1.2 格式化与 Lint
在提交任何 Pull Request 或 Commit 之前，必须确保通过以下格式化与静态检查：

```powershell
# 1. 自动代码格式化
cargo fmt --all

# 2. 检查格式化规范
cargo fmt --all -- --check

# 3. Clippy 静态检查
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 2. 提交信息规范 (Conventional Commits)

Commit 信息应遵循清晰、结构化的语义化格式：

```text
<type>(<scope>): <subject>

[可选 body]
[可选 footer]
```

### 常用 `type` 类型：
- `feat`: 新增功能特性（如新增渲染 Pass、支持新硬件特性）
- `fix`: 修复 Bug 或异常行为
- `docs`: 文档变更或补充
- `style`: 代码格式化调整（不影响代码逻辑的空白、标点等）
- `refactor`: 代码重构（既不是新增功能也不是修复 bug）
- `perf`: 性能优化
- `test`: 新增或修改测试用例
- `chore`: 构建系统、依赖更新或辅助工具变动

**示例**：
```text
feat(render): integrate DXR 1.1 inline raytracing pipeline
fix(diligent-rs): resolve device context null pointer dereference
docs(readme): add detailed DX12U feature matrix and build instructions
```

---

## 3. 分支与 Pull Request 流程

1. **Fork** 本仓库至个人 GitHub 空间。
2. 从 `main` 分支切出功能分支：
   ```bash
   git checkout -b feat/your-feature-name
   ```
3. 在本地编写代码、补充必要的单元测试与文档。
4. 运行 `cargo fmt` 与 `cargo clippy` 确保无报错。
5. 提交变更并推送至个人远程分支。
6. 发起 Pull Request，详细描述修改动机、技术设计与测试验证结果。
