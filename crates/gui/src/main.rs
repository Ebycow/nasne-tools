#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! nasne-gui: nasne の番組表・録画予約・録画管理のデスクトップアプリ
//!
//! 起動、ウィンドウ、日本語フォントの読み込みと配色の設定。画面は app.rs。

mod app;
mod theme;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle, Theme};
use std::sync::Arc;
use std::time::Instant;

/// OS 標準の日本語フォント（mmap で開くので起動時に読み込みは発生しない）
fn font_candidates() -> (Vec<String>, Vec<String>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        (
            s(&[
                "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
                "/Library/Fonts/Arial Unicode.ttf",
            ]),
            s(&["/System/Library/Fonts/ヒラギノ角ゴシック W6.ttc"]),
        )
    } else if cfg!(windows) {
        let w = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let f = |n: &str| format!("{w}\\Fonts\\{n}");
        (
            vec![f("YuGothM.ttc"), f("YuGothR.ttc"), f("meiryo.ttc"), f("msgothic.ttc")],
            vec![f("YuGothB.ttc"), f("meiryob.ttc")],
        )
    } else {
        let mut reg = s(&[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/OTF/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
            "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
            "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
            "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
            "/usr/share/fonts/truetype/vlgothic/VL-PGothic-Regular.ttf",
        ]);
        let bold = s(&[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/OTF/NotoSansCJK-Bold.ttc",
        ]);
        if !reg.iter().any(|p| std::path::Path::new(p).exists())
            && let Ok(o) = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans:lang=ja"]).output()
        {
            reg.push(String::from_utf8_lossy(&o.stdout).trim().to_string());
        }
        (reg, bold)
    }
}

fn map_font(path: &str) -> Option<FontData> {
    let f = std::fs::File::open(path).ok()?;
    // SAFETY: OS のフォントファイルは実行中に書き換えられない前提
    let m = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    Some(FontData::from_blob(Arc::new(m), 0))
}

fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let (reg, bold) = font_candidates();
    let regular = reg.iter().find_map(|p| map_font(p));
    let bolder = bold.iter().find_map(|p| map_font(p));
    let prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut bold_list = Vec::new();
    if let Some(r) = regular {
        fonts.font_data.insert("jp".into(), Arc::new(r));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().insert(0, "jp".into());
        }
    }
    if let Some(b) = bolder {
        fonts.font_data.insert("jp_bold".into(), Arc::new(b));
        bold_list.push("jp_bold".to_string());
    }
    if fonts.font_data.contains_key("jp") {
        bold_list.push("jp".into());
    }
    bold_list.extend(prop);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_list);
    ctx.set_fonts(fonts);

    for (theme, p) in [(Theme::Light, theme::LIGHT), (Theme::Dark, theme::DARK)] {
        ctx.style_mut_of(theme, |s| {
            s.text_styles = [
                (TextStyle::Small, FontId::proportional(11.0)),
                (TextStyle::Body, FontId::proportional(14.0)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Heading, FontId::new(17.0, FontFamily::Name("bold".into()))),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();
            s.spacing.button_padding = egui::vec2(10.0, 4.0);
            s.spacing.interact_size.y = 26.0;
            let v = &mut s.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.panel;
            v.extreme_bg_color = p.panel;
            v.faint_bg_color = p.bg;
            v.hyperlink_color = p.accent;
            v.selection.bg_fill = p.accent.gamma_multiply(0.35);
            v.selection.stroke = egui::Stroke::new(1.0, p.accent);
            v.override_text_color = Some(p.text);
            let w = &mut v.widgets;
            for (st, fill) in [
                (&mut w.inactive, p.panel),
                (&mut w.hovered, p.panel),
                (&mut w.active, p.accent_soft),
                (&mut w.open, p.panel),
            ] {
                st.weak_bg_fill = fill;
                st.bg_fill = fill;
                st.corner_radius = egui::CornerRadius::same(6);
            }
            w.inactive.bg_stroke = egui::Stroke::new(1.0, p.line);
            w.hovered.bg_stroke = egui::Stroke::new(1.0, p.muted);
            w.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
            w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.line);
        });
    }
}

fn main() -> eframe::Result {
    let started = Instant::now();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nasne")
            .with_app_id("nasne-gui")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "nasne-gui",
        opts,
        Box::new(move |cc| {
            let bench = std::env::var_os("NASNE_GUI_BENCH").is_some();
            if bench {
                eprintln!("ウィンドウ作成: {:?}", started.elapsed());
            }
            setup(&cc.egui_ctx);
            if bench {
                eprintln!("フォント設定: {:?}", started.elapsed());
            }
            Ok(Box::new(app::App::new(cc, started)))
        }),
    )
}
