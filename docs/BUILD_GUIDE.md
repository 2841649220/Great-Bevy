# Great-Bevy 全平台编译与开发环境配置指南

本文档提供在 Windows、Linux 等主流平台上从零编译、构建以及调试 **Great-Bevy** 引擎的完整指南。

---

## 1. 软件环境要求

### 1.1 Windows 环境（主打平台，验证通过）

| 组件 | 最低要求 | 推荐配置 | 说明 |
|------|----------|----------|------|
| **操作系统** | Windows 10 (1903+) / Windows 11 | Windows 11 (23H2+) | 支持 DXR 1.1 与 DX12 Ultimate 特性 |
| **Rust 工具链** | Rust 1.85.0+ | `stable-x86_64-pc-windows-msvc` | 使用 2024 Edition 标准 |
| **Visual Studio** | VS 2019+ | VS 2022 (MSVC v143) | 必须勾选 "C++ 桌面开发" 工作负载 |
| **Windows SDK** | 10.0.19041.0 | 10.0.22621.0 或更高 | 需提供 `d3d12.h` 与 `dxgi1_6.h` |
| **CMake** | 3.20+ | 3.28+ | 构建 DiligentCore 原生库 |
| **Ninja** | 1.10+ | 最新版 | 大幅提升 CMake 编译速度 |
| **LLVM / Clang** | 15.0+ | 17.0+ | 用于 `bindgen` 生成 FFI 头文件映射 |

---

## 2. 依赖项安装与配置

### 2.1 安装 Rust 工具链
```powershell
# 安装 Rustup 并切换至 MSVC 目标
rustup default stable-x86_64-pc-windows-msvc
rustup update
```

### 2.2 安装编译工具（通过 Winget / Chocolatey / Scoop）
```powershell
# 通过 Winget 安装 CMake, Ninja 与 LLVM
winget install Kitware.CMake
winget install Ninja-build.Ninja
winget install LLVM.LLVM
```

### 2.3 环境变量校验
请确保以下路径已加入系统 `PATH` 或设置对应环境变量：
```powershell
# 验证工具链
cmake --version
ninja --version
clang --version
cargo --version

# 可选：显式指定 LIBCLANG_PATH（若 bindgen 无法自动定位）
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
```

---

## 3. 项目编译与运行

### 3.1 编译与运行 3D DXR 光线追踪与 DX12U 演示
```powershell
# 调试模式（构建速度快）
cargo run --example test_demo_3d

# 发行版极速模式（帧率与性能优化最佳）
cargo run --release --example test_demo_3d
```

### 3.2 编译与运行 2D 几何与发光粒子流演示
```powershell
cargo run --example test_demo_2d
```

### 3.3 运行核心子系统回归验证（Headless）
```powershell
cargo run --example test_demo
```

### 3.4 独立构建 `diligent-sys` 原生 FFI 绑定
```powershell
cargo build --manifest-path crates/diligent-sys/Cargo.toml
```

---

## 4. 高级编译选项与环境变量

| 环境变量 | 作用与用法 | 默认值 |
|----------|------------|--------|
| `DILIGENT_SKIP_CMAKE` | 设为 `1` 时跳过原生 CMake 编译，仅运行 `bindgen` 绑定生成（降级排查模式） | `0` |
| `DILIGENT_CMAKE` | 手动指定 `cmake.exe` 的绝对路径 | 自动从 `PATH` 发现 |
| `DILIGENT_NINJA` | 手动指定 `ninja.exe` 的绝对路径 | 自动从 `PATH` 发现 |
| `DILIGENT_MSVC_TOOLS_DIR` | 手动指定 MSVC 工具链目录（如 `VC/Tools/MSVC/14.xx.xx`） | 自动通过 `vswhere` 定位 |
| `DILIGENT_WINDOWS_KITS_DIR` | 手动指定 Windows SDK 目录（如 `Windows Kits/10`） | 自动从注册表定位 |

---

## 5. 常见问题排查 (Troubleshooting)

### Q1: `bindgen` 报错 `Unable to find libclang`
**解决方法**：
1. 确认已安装 LLVM：`winget install LLVM.LLVM`。
2. 设置环境变量：
   ```powershell
   $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
   ```

### Q2: 找不到 MSVC 编译工具或 Windows SDK 头文件
**解决方法**：
1. 打开 Visual Studio Installer，确认已勾选：
   - "MSVC v143 - VS 2022 C++ x64/x86 生成工具"
   - "Windows 10/11 SDK (10.0.22621.0 或更高版本)"
2. 在 PowerShell 中使用 "Developer Command Prompt for VS 2022" 运行编译。

### Q3: 离线构建时 ThirdParty 子模块拉取超时
**解决方法**：
`diligent-sys` 的 `build.rs` 内置了基于 `codeload.github.com` 的自动回填与 SHA-256 完整性校验机制。若网络受限，可手动将所需模块解压至 `third_party/DiligentEngine/ThirdParty/` 对应子目录即可。

---

## 6. 代码质量与格式化检查

在提交代码前，建议运行以下检查脚本确保代码风格符合标准：

```powershell
# 1. 代码格式化校验
cargo fmt --all -- --check

# 2. 静态分析与 Lint 检查
cargo clippy --workspace --all-targets -- -D warnings
```
