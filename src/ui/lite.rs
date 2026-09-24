//! lite 版 Material Design 界面壳。
//!
//! 结构：顶部应用栏 + 底部导航（首页 / 设置 / 日志 / 关于）四个子页面，
//! 内容按 Material 卡片组织。未登录时首页即登录页；登录后首页只保留
//! 本次跑步方案与「开始跑步」主按钮，全部参数收进「设置」，日志与
//! 更新独立成页。桌面端加 `lite` feature 时同样生效。

use super::{mobile, theme, App};
use crate::platform::InputKind;
use chrono::TimeZone;
use eframe::egui;
use egui::{Align, Color32, Layout, RichText, Stroke};

// Material 3 浅色 tokens（青绿主色，与标准版共用 theme 基调）
const PRIMARY: Color32 = Color32::from_rgb(13, 148, 136);
const ON_PRIMARY: Color32 = Color32::WHITE;
const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(202, 238, 233);
const ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(4, 78, 72);
const CARD: Color32 = Color32::WHITE;
const OUTLINE: Color32 = Color32::from_rgb(206, 219, 214);
const SURFACE_CONTAINER: Color32 = Color32::from_rgb(235, 241, 238);
const OK_CONTAINER: Color32 = Color32::from_rgb(206, 240, 224);

/// Material 卡片：白底大圆角 + 轻投影，可带小节标题（空串省略）。
fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(CARD)
        .rounding(egui::Rounding::same(16.0))
        .shadow(egui::Shadow {
            offset: egui::vec2(0.0, 2.0),
            blur: 8.0,
            spread: 0.0,
            color: Color32::from_black_alpha(16),
        })
        .inner_margin(egui::Margin::same(14.0))
        .outer_margin(egui::Margin::symmetric(12.0, 6.0))
        .show(ui, |ui| {
            let r = if title.is_empty() {
                add(ui)
            } else {
                ui.label(RichText::new(title).strong().size(15.0).color(theme::text()));
                ui.add_space(6.0);
                add(ui)
            };
            r
        })
        .inner
}

/// Material 填充按钮（主操作）。
fn filled_btn(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(ON_PRIMARY).strong())
        .fill(PRIMARY)
        .stroke(Stroke::NONE)
        .rounding(egui::Rounding::same(26.0))
}

/// Material 色调按钮（次级操作）。
fn tonal_btn(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(ON_PRIMARY_CONTAINER).strong())
        .fill(PRIMARY_CONTAINER)
        .stroke(Stroke::NONE)
        .rounding(egui::Rounding::same(26.0))
}

/// 状态胶囊（顶栏登录态用）。
fn chip(ui: &mut egui::Ui, text: String, bg: Color32, fg: Color32) {
    egui::Frame::none()
        .fill(bg)
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::symmetric(10.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.5).color(fg));
        });
}

/// 小指标（说明文字 + 数值）。
fn stat(ui: &mut egui::Ui, caption: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(caption).small().color(theme::text_dim()));
        ui.label(RichText::new(value).strong().size(16.0));
    });
}

fn fmt_pace(s: f32) -> String {
    format!("{}:{:02}/km", (s / 60.0) as i64, (s as i64) % 60)
}

fn fmt_dur(s: i64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn days_ago_label(days_ago: i64) -> String {
    match days_ago {
        0 => "今天".into(),
        1 => "昨天".into(),
        n => format!("{n} 天前"),
    }
}

// ── 界面壳 ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Default)]
enum Page {
    #[default]
    Home,
    Settings,
    Log,
    About,
}

#[derive(Default)]
pub struct LiteUi {
    page: Page,
}

/// lite 版整帧绘制（顶栏 + 底部导航 + 子页面）。
pub fn draw_app(app: &mut App, ctx: &egui::Context) {
    draw_top_bar(app, ctx);
    egui::TopBottomPanel::bottom("lite_nav_bar").show(ctx, |ui| {
        nav(ui, &mut app.lite.page);
    });
    let page = app.lite.page;
    egui::CentralPanel::default().show(ctx, |ui| match page {
        Page::Home => home(app, ui),
        Page::Settings => settings(app, ui),
        Page::Log => log_page(app, ui),
        Page::About => about_page(app, ui),
    });
}

fn draw_top_bar(app: &App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("lite_top_bar").show(ctx, |ui| {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("校园跑助手").size(20.0).strong());
            ui.label(RichText::new("lite").size(12.0).color(theme::text_dim()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match &app.session {
                    Some(s) => chip(
                        ui,
                        format!("● {}", s.name),
                        OK_CONTAINER,
                        ON_PRIMARY_CONTAINER,
                    ),
                    None => chip(ui, "未登录".to_owned(), SURFACE_CONTAINER, theme::text_dim()),
                }
            });
        });
        ui.add_space(6.0);
    });
}

/// Material 3 底部导航（文字标签版）。返回各项响应供离屏布局测试断言。
fn nav(ui: &mut egui::Ui, page: &mut Page) -> Vec<egui::Response> {
    let mut responses = Vec::new();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let items = [
            (Page::Home, "首页"),
            (Page::Settings, "设置"),
            (Page::Log, "日志"),
            (Page::About, "关于"),
        ];
        let n = items.len() as f32;
        let spacing = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - spacing * (n - 1.0)) / n;
        for (target, label) in items {
            let selected = *page == target;
            let btn = if selected {
                egui::Button::new(
                    RichText::new(label).size(13.5).color(ON_PRIMARY_CONTAINER).strong(),
                )
                .fill(PRIMARY_CONTAINER)
                .stroke(Stroke::NONE)
                .rounding(egui::Rounding::same(22.0))
            } else {
                egui::Button::new(RichText::new(label).size(13.5).color(theme::text_dim()))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .rounding(egui::Rounding::same(22.0))
            };
            responses.push(ui.add_sized([width, 42.0], btn));
            if responses.last().map(|r| r.clicked()).unwrap_or(false) {
                *page = target;
            }
        }
    });
    ui.add_space(6.0);
    responses
}

// ── 首页 ────────────────────────────────────────────────────

fn home(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.session.is_none() {
                login_card(app, ui);
            } else {
                plan_hero(app, ui);
                app.draw_run_warnings(ui);
                start_card(app, ui);
            }
            status_card(app, ui);
        });
}

fn ip_line(app: &App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("网络").small().color(theme::text_dim()));
        if app.ip.is_empty() {
            ui.label(RichText::new("获取中…").small().color(theme::warn()));
        } else if app.ip == "失败" {
            ui.label(RichText::new("IP 获取失败").small().color(theme::err()));
        } else {
            ui.monospace(RichText::new(&app.ip).small());
        }
    });
}

/// 未登录首页：登录卡片。
fn login_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "登录账号", |ui| {
        ip_line(app, ui);
        ui.add_space(8.0);
        ui.label("账号");
        mobile::text_edit(
            ui,
            "login_username",
            &mut app.username,
            InputKind::Text,
            ui.available_width(),
        );
        ui.add_space(2.0);
        ui.label("密码");
        mobile::text_edit(
            ui,
            "login_password",
            &mut app.password,
            InputKind::Password,
            ui.available_width(),
        );
        ui.add_space(2.0);
        ui.checkbox(&mut app.remember, "记住密码");
        ui.add_space(8.0);
        let btn = if app.login_busy {
            egui::Button::new(RichText::new("登录中…").color(ON_PRIMARY))
                .fill(PRIMARY)
                .stroke(Stroke::NONE)
                .rounding(egui::Rounding::same(26.0))
        } else {
            filled_btn("登录")
        };
        if ui
            .add_sized([ui.available_width(), 50.0], btn)
            .clicked()
            && !app.login_busy
        {
            app.do_login();
        }
    });
}

/// 本次跑步方案：大号距离 + 指标行 + 换一版。
fn plan_hero(app: &mut App, ui: &mut egui::Ui) {
    app.run_page.ensure_plan();
    let Some(p) = app.run_page.plan.clone() else {
        return;
    };
    let start = chrono::Local
        .timestamp_millis_opt(p.start_ms)
        .single()
        .map(|t| t.format("%m-%d %H:%M").to_string())
        .unwrap_or_default();
    card(ui, "", |ui| {
        ui.label(RichText::new("本次跑步计划").small().color(theme::text_dim()));
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{:.2}", p.dist))
                    .size(34.0)
                    .strong()
                    .color(theme::accent()),
            );
            ui.label(RichText::new("km").size(14.0).color(theme::text_dim()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(tonal_btn("换一版")).clicked() {
                    app.run_page.regen_plan();
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            stat(ui, "配速", &fmt_pace(p.pace));
            ui.separator();
            stat(ui, "用时", &fmt_dur(p.dur));
            ui.separator();
            stat(ui, "开始", &start);
        });
    });
}

fn start_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "", |ui| {
        let busy = app.run_busy;
        let btn = if busy { filled_btn("提交中…") } else { filled_btn("开始跑步") };
        let enabled = !busy && app.session.is_some();
        if ui
            .add_sized([ui.available_width(), 54.0], btn)
            .clicked()
            && enabled
        {
            app.start_run();
        }
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("跑步数据上传中，请保持网络畅通").small().color(theme::text_dim()));
            });
        }
    });
}

/// 网络与定位状态卡片（登录前后都显示）。
fn status_card(app: &App, ui: &mut egui::Ui) {
    card(ui, "", |ui| {
        ip_line(app, ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("定位").small().color(theme::text_dim()));
            if app.identity.location_from_gps {
                ui.label(
                    RichText::new(format!(
                        "● {} ({:.5},{:.5})",
                        app.identity.city, app.identity.anchor_lat, app.identity.anchor_lon
                    ))
                    .small()
                    .color(theme::ok()),
                );
            } else if app.identity.has_unconfigured_default_location() {
                ui.label(
                    RichText::new("等待本机定位授权…")
                        .small()
                        .color(theme::warn()),
                );
            } else {
                ui.label(RichText::new(&app.identity.city).small());
            }
        });
        if let Some(s) = &app.data_page.semester {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("学期").small().color(theme::text_dim()));
                ui.label(
                    RichText::new(format!(
                        "{}：有效 {}/{} 次",
                        s.sname, s.semester_valid_count, s.semester_count
                    ))
                    .small(),
                );
            });
        }
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);
        ui.label(RichText::new(&app.status).small().color(theme::plain()));
    });
}

// ── 设置 ────────────────────────────────────────────────────

fn settings(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            params_card(app, ui);
            time_card(app, ui);
            account_card(app, ui);
        });
}

fn params_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "运动参数", |ui| {
        let page = &mut app.run_page;
        ui.label(RichText::new("距离范围（km）").color(theme::text_dim()));
        ui.horizontal(|ui| {
            mobile::drag_f32(ui, "run_dist_min", &mut page.dist_min, 0.5..=20.0, 0.05, 2, " km");
            ui.label("至");
            mobile::drag_f32(ui, "run_dist_max", &mut page.dist_max, 0.5..=20.0, 0.05, 2, " km");
        });
        ui.add_space(2.0);
        ui.label(RichText::new("配速范围（秒/公里）").color(theme::text_dim()));
        ui.horizontal(|ui| {
            mobile::drag_f32(ui, "run_pace_min", &mut page.pace_min, 180.0..=520.0, 5.0, 0, "");
            ui.label("至");
            mobile::drag_f32(ui, "run_pace_max", &mut page.pace_max, 180.0..=520.0, 5.0, 0, "");
        });
        ui.add_space(2.0);
        ui.label(RichText::new("海拔范围（米，留空自动）").color(theme::text_dim()));
        ui.horizontal(|ui| {
            mobile::text_edit(
                ui,
                "run_manual_altitude_min",
                &mut page.manual_altitude_min,
                InputKind::Text,
                100.0,
            );
            ui.label("-");
            mobile::text_edit(
                ui,
                "run_manual_altitude_max",
                &mut page.manual_altitude_max,
                InputKind::Text,
                100.0,
            );
        });
        ui.add_space(4.0);
        ui.checkbox(&mut page.face_check, "提交时标记人脸校验（faceCheck=1）");
    });
}

fn time_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "开始时间", |ui| {
        let page = &mut app.run_page;
        // 分段按钮：随机时刻 / 指定时刻
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 8.0) / 2.0;
            for (mode, label) in [(0usize, "随机时刻"), (1usize, "指定时刻")] {
                let selected = page.start_mode == mode;
                let btn = if selected {
                    egui::Button::new(RichText::new(label).color(ON_PRIMARY_CONTAINER).strong())
                        .fill(PRIMARY_CONTAINER)
                        .stroke(Stroke::NONE)
                        .rounding(egui::Rounding::same(22.0))
                } else {
                    egui::Button::new(RichText::new(label).color(theme::text_dim()))
                        .fill(CARD)
                        .stroke(Stroke::new(1.0_f32, OUTLINE))
                        .rounding(egui::Rounding::same(22.0))
                };
                if ui.add_sized([half, 40.0], btn).clicked() {
                    page.start_mode = mode;
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("日期");
            egui::ComboBox::from_id_salt("run_days_ago")
                .width(96.0)
                .selected_text(days_ago_label(page.days_ago))
                .show_ui(ui, |ui| {
                    for d in 0..=3 {
                        ui.selectable_value(&mut page.days_ago, d, days_ago_label(d));
                    }
                });
            if page.start_mode == 0 {
                ui.label(
                    RichText::new("7:00-20:00 内随机")
                        .small()
                        .color(theme::text_dim()),
                );
            }
        });
        if page.start_mode == 1 {
            ui.horizontal_wrapped(|ui| {
                ui.label("时刻");
                mobile::drag_i64(ui, "run_hour", &mut page.hour, 0..=23, 1.0, "", " 点");
                ui.label(":");
                mobile::drag_i64(ui, "run_minute", &mut page.minute, 0..=59, 1.0, "", " 分");
            });
        }
    });
}

fn account_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "账号与设备", |ui| {
        match &app.session {
            Some(s) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("账号").small().color(theme::text_dim()));
                    ui.label(RichText::new(format!("{}（uid={}）", s.name, s.uid)).small());
                });
            }
            None => {
                ui.label(RichText::new("未登录").small().color(theme::err()));
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("设备").small().color(theme::text_dim()));
            ui.monospace(RichText::new(&app.identity.device_id).small());
        });
        ip_line(app, ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("定位").small().color(theme::text_dim()));
            if app.identity.location_from_gps {
                ui.label(
                    RichText::new(format!(
                        "● {} ({:.5},{:.5})",
                        app.identity.city, app.identity.anchor_lat, app.identity.anchor_lon
                    ))
                    .small()
                    .color(theme::ok()),
                );
            } else {
                ui.label(RichText::new(&app.identity.city).small());
            }
        });
        ui.add_space(6.0);
        if app.session.is_some() {
            let logout = egui::Button::new(RichText::new("登出账号").color(theme::err()))
                .fill(CARD)
                .stroke(Stroke::new(1.0_f32, OUTLINE))
                .rounding(egui::Rounding::same(22.0));
            if ui
                .add_sized([ui.available_width(), 44.0], logout)
                .clicked()
            {
                app.do_logout();
            }
        }
    });
}

// ── 日志 ────────────────────────────────────────────────────

fn log_page(app: &mut App, ui: &mut egui::Ui) {
    card(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("运行日志").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("清空").clicked() {
                    app.log.clear();
                }
                ui.label(
                    RichText::new(format!("{} 行", app.log.len()))
                        .small()
                        .color(theme::text_dim()),
                );
            });
        });
    });
    app.log.render(ui);
}

// ── 关于 ────────────────────────────────────────────────────

fn about_page(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            card(ui, "版本与更新", |ui| {
                app.draw_about(ui);
            });
            card(ui, "声明", |ui| {
                ui.label(
                    RichText::new(
                        "本工具仅供学习研究，请勿用于违反学校规定的场景。\n许可证：CC BY-NC 4.0（禁止商用）。",
                    )
                    .small()
                    .color(theme::text_dim()),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 底部导航在手机/桌面宽度下都不越界且保持触控高度。
    #[test]
    fn nav_bar_fits_phone_and_desktop_widths() {
        for width in [320.0, 360.0, 412.0, 880.0] {
            let ctx = egui::Context::default();
            let mut rects = Vec::new();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 720.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::TopBottomPanel::bottom("lite_nav_bar").show(ctx, |ui| {
                    let mut page = Page::Home;
                    rects = nav(ui, &mut page).into_iter().map(|r| r.rect).collect();
                });
            });
            assert_eq!(rects.len(), 4, "width={width}");
            for rect in &rects {
                assert!(rect.is_finite() && rect.width() > 0.0, "width={width}: {rect:?}");
                assert!(rect.max.x <= width + 0.5, "width={width}: {rects:?}");
                assert!(rect.height() >= 40.0, "width={width}: {rect:?}");
            }
        }
    }

    /// Material 卡片与其中的分段按钮/下拉框在手机宽度内不越界。
    #[test]
    fn card_and_segmented_fit_narrow_widths() {
        for width in [320.0, 360.0] {
            let ctx = egui::Context::default();
            let mut rects = Vec::new();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 720.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    card(ui, "开始时间", |ui| {
                        ui.horizontal(|ui| {
                            let half = (ui.available_width() - 8.0) / 2.0;
                            for label in ["随机时刻", "指定时刻"] {
                                let btn = egui::Button::new(RichText::new(label))
                                    .fill(CARD)
                                    .stroke(Stroke::new(1.0_f32, OUTLINE))
                                    .rounding(egui::Rounding::same(22.0));
                                rects.push(ui.add_sized([half, 40.0], btn).rect);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("日期");
                            let mut days_ago = 0i64;
                            egui::ComboBox::from_id_salt("test_days_ago")
                                .width(96.0)
                                .selected_text("今天")
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut days_ago, 0, "今天");
                                });
                        });
                        rects.push(ui.response().rect);
                    });
                });
            });
            for rect in &rects {
                assert!(rect.max.x <= width + 0.5, "width={width}: {rects:?}");
            }
        }
    }
}
