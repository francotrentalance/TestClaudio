//! Colors, fonts and widget styling.

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle, Visuals,
};
use holdem_trainer::grading::Grade;

pub const BG: Color32 = Color32::from_rgb(12, 20, 34);
pub const PANEL: Color32 = Color32::from_rgb(20, 31, 50);
pub const PANEL_2: Color32 = Color32::from_rgb(28, 42, 66);
pub const PANEL_3: Color32 = Color32::from_rgb(38, 55, 84);
pub const BORDER: Color32 = Color32::from_rgb(44, 62, 94);
pub const TEXT: Color32 = Color32::from_rgb(232, 238, 247);
pub const TEXT_DIM: Color32 = Color32::from_rgb(146, 162, 189);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(122, 138, 168);
pub const GOLD: Color32 = Color32::from_rgb(245, 184, 61);
pub const TEAL: Color32 = Color32::from_rgb(61, 214, 198);
pub const FLAME: Color32 = Color32::from_rgb(255, 122, 69);

pub const FELT: Color32 = Color32::from_rgb(22, 110, 72);
pub const FELT_DARK: Color32 = Color32::from_rgb(14, 78, 52);
pub const RAIL: Color32 = Color32::from_rgb(64, 44, 31);
pub const RAIL_LIGHT: Color32 = Color32::from_rgb(98, 70, 48);

pub const FOLD: Color32 = Color32::from_rgb(74, 135, 222);
pub const PASSIVE: Color32 = Color32::from_rgb(46, 173, 107);
pub const BET_S: Color32 = Color32::from_rgb(240, 138, 36);
pub const BET_L: Color32 = Color32::from_rgb(229, 83, 61);
pub const ALLIN: Color32 = Color32::from_rgb(168, 72, 200);
/// Range grid cells (or the part of a cell) the range does not reach.
pub const OUT_OF_RANGE: Color32 = Color32::from_rgb(31, 39, 54);

pub const RED_SUIT: Color32 = Color32::from_rgb(214, 40, 57);
pub const BLUE_SUIT: Color32 = Color32::from_rgb(28, 104, 200);
pub const GREEN_SUIT: Color32 = Color32::from_rgb(24, 140, 74);
pub const BLACK_SUIT: Color32 = Color32::from_rgb(28, 30, 38);

pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";
pub const MEDIUM: &str = "medium";

pub fn grade_color(g: Grade) -> Color32 {
    match g {
        Grade::Perfect => Color32::from_rgb(61, 220, 132),
        Grade::Good => Color32::from_rgb(150, 210, 80),
        Grade::Inaccuracy => GOLD,
        Grade::Mistake => Color32::from_rgb(240, 122, 61),
        Grade::Blunder => Color32::from_rgb(229, 72, 77),
    }
}

pub fn grade_icon(g: Grade) -> &'static str {
    match g {
        Grade::Perfect => "★",
        Grade::Good => "✔",
        Grade::Inaccuracy => "!",
        Grade::Mistake => "✖",
        Grade::Blunder => "✖✖",
    }
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

/// Mixes `a` toward `b` by `t` (0..1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 4] = [
        ("poppins", include_bytes!("../../assets/fonts/Poppins-Regular.ttf")),
        ("poppins-medium", include_bytes!("../../assets/fonts/Poppins-Medium.ttf")),
        ("poppins-semibold", include_bytes!("../../assets/fonts/Poppins-SemiBold.ttf")),
        ("poppins-bold", include_bytes!("../../assets/fonts/Poppins-Bold.ttf")),
    ];
    for (name, bytes) in faces {
        fonts.font_data.insert(name.to_string(), FontData::from_static(bytes));
    }
    // Keep egui's fonts as fallbacks for symbols and emoji.
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |main: &str| {
        let mut v = vec![main.to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("poppins"));
    fonts.families.insert(FontFamily::Name(MEDIUM.into()), family("poppins-medium"));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), family("poppins-semibold"));
    fonts.families.insert(FontFamily::Name(BOLD.into()), family("poppins-bold"));
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, semibold(24.0)),
        (TextStyle::Body, font(15.0)),
        (TextStyle::Button, medium(15.0)),
        (TextStyle::Small, font(12.5)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.slider_width = 220.0;

    let mut v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(9, 15, 26);
    v.faint_bg_color = PANEL;
    v.selection.bg_fill = mix(TEAL, BG, 0.45);
    v.selection.stroke = Stroke::new(1.0_f32, TEAL);
    v.hyperlink_color = TEAL;
    let round = Rounding::same(8.0);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, PANEL),
        (&mut v.widgets.inactive, PANEL_2),
        (&mut v.widgets.hovered, PANEL_3),
        (&mut v.widgets.active, mix(PANEL_3, TEAL, 0.25)),
        (&mut v.widgets.open, PANEL_3),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.rounding = round;
        w.bg_stroke = Stroke::new(1.0_f32, BORDER);
        w.fg_stroke = Stroke::new(1.5_f32, TEXT);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, mix(BORDER, TEAL, 0.5));
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = Rounding::same(8.0);
    style.visuals = v;
    ctx.set_style(style);
}
