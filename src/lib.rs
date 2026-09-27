//! Shared desktop and Android application.

mod api;
pub mod cli;
mod crypto;
pub mod location;
pub mod platform;
pub mod template;
// 特供 lite 版不拉取更新：update/relaunch 的调用点全部按 cfg 剔除，
// 模块本体保留（消息协议仍引用 ReleaseInfo 类型），故 lite 下允许 dead code。
#[cfg_attr(feature = "lite", allow(dead_code))]
mod relaunch;
mod textlog;
mod track;
#[cfg_attr(feature = "lite", allow(dead_code))]
mod update;

#[cfg(feature = "gui")]
mod ui;

#[cfg(all(target_os = "android", feature = "android"))]
mod android;

#[cfg(feature = "gui")]
pub fn run_gui(options: eframe::NativeOptions) -> eframe::Result<()> {
    eframe::run_native(
        "NekoSportsWorldTool",
        options,
        Box::new(|cc| {
            #[cfg(target_os = "android")]
            android::set_context(&cc.egui_ctx);
            Ok(Box::new(ui::App::new(cc)))
        }),
    )
}
