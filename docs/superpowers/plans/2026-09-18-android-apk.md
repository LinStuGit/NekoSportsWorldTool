# Android APK Implementation Plan

> **For agentic workers:** Use subagent-driven-development for the bounded mobile UI task and review. Continue implementation in this session; the user has agreed to the product scope.

**Goal:** 交付可在 Android 16 ARM64 手机上安装的前台版 APK。

**Architecture:** 共用 Rust 业务与 egui 页面，薄 Android NativeActivity 外壳提供默认输入法和屏幕常亮。独立工作区保留原 main 及本地运行数据。

**Tech Stack:** Rust, eframe/egui 0.29.1, winit 0.30, Android SDK/NDK, Java, JNI.

## Global Constraints

- 面向 Android 16；暂按 ARM64 手机制作，可另构建模拟器版本用于验证。
- 保留全部现有功能及 egui 界面风格，适配竖屏、触控与软键盘。
- 仅要求 App 打开并位于前台时可用，不增加后台服务、后台调度或锁屏运行保证。
- 调用手机当前默认输入法；支持中文组合输入、密码隐藏和键盘收起。
- 用户启动的长任务运行期间保持屏幕常亮，任务结束后释放；切到后台不持有唤醒锁。
- 不把原工作区的会话、配置、设备标识打包或用于自动业务请求。

## Task 1: 可移植入口与存储

Files: `src/main.rs`, `src/lib.rs`, `src/platform.rs`, `src/api/model.rs`, `src/api/rank.rs`, `Cargo.toml`.

- [x] 运行 `cargo test --locked --no-default-features` 获得基线。
- [x] 增加数据目录行为测试：注入目录后创建、读取、清除会话都只作用于该目录；未初始化 Android 目录时显式失败，不能悄悄写入系统目录。
- [x] 提取可共用库入口，桌面二进制调用该库。
- [x] 提供 `platform::data_dir() -> PathBuf`，Android 初始化期间设置一次；将两处 current_exe 持久化改为共用目录。
- [x] Windows GUI、CLI 编译与离线测试通过。

## Task 2: Android 外壳与默认输入法

Files: `src/android.rs`, `android/AndroidManifest.xml`, `android/java/org/nekosportsworld/tool/MainActivity.java`, `android/build.ps1`, `Cargo.toml`.

- [x] 建立 `cdylib` Android 入口；使用 eframe EventLoopBuilderHook 注入 AndroidApp。
- [x] Java NativeActivity 提供原生 EditText 对话编辑层，接受 ID、初始文本、密码/数值类型，通过 JNI 回传确认结果。
- [x] 输入桥接口：`platform::edit_text(id: u64, value: &str, kind: InputKind)`，`platform::take_edited_text(id: u64) -> Option<String>`；取消不更改原值。
- [x] `platform::set_keep_screen_on(bool)` 在状态变化时更新 Activity Window 标志。
- [x] Android TLS 不启用桌面 native-certs；保留受信根证书校验。
- [x] 构建脚本使用 SDK 工具编译 Java、打包共享库、zipalign、apksigner，密钥保存在忽略目录。

## Task 3: 手机 UI 与中文字体

Files: `src/ui/*.rs`, `assets/fonts/*`.

- [x] 将桌面横向布局在窄屏下折行或改为纵向；全部七页面及所有操作保留。
- [x] 文本编辑控件在 Android 使用 Task 2 输入桥，在桌面使用原 egui 输入；数值输入提供移动编辑入口。
- [x] 任务运行状态驱动常亮，任务结束或退出释放。
- [x] 装载可再分发中文字体并保留许可证。
- [x] 用 egui 离屏帧检查 360/412/880 宽度布局，检测内容越界与无效矩形。

## Task 4: 打包与验收

Files: `android/README.md`, build outputs in ignored `android/build/`.

- [x] 构建 ARM64 APK，签名验证及 ELF/ZIP 16 KB 对齐检查。
- [x] 条件允许时构建 x86_64 版本并启动 Android 模拟器验证 UI/输入；不会自动调用真实账号业务操作。
- [x] 独立审阅完整修改，修正重要问题，再执行针对性的回归检查。
- [x] 写明安装方式、验证证据和仍需真机确认的项目，交付 APK 与可复现脚本。
