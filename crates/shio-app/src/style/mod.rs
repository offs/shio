mod button;
mod control;

pub(crate) use button::{Button, button};
pub(crate) use control::{
    checkbox, pick_list, pick_list_menu, progress, scrollable, slider, text_editor, text_input,
    toggler,
};

use iced::widget::container;
use iced::{Border, Color, Shadow, Theme, Vector};

pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_S: f32 = 8.0;
pub(crate) const SPACE_M: f32 = 12.0;
pub(crate) const SPACE_L: f32 = 16.0;
pub(crate) const SPACE_XL: f32 = 24.0;

pub(crate) const TEXT_CAPTION: f32 = 11.0;
pub(crate) const TEXT_SMALL: f32 = 12.0;
pub(crate) const TEXT_BODY: f32 = 13.0;
pub(crate) const TEXT_TITLE: f32 = 14.0;
pub(crate) const TEXT_DISPLAY: f32 = 18.0;

const RADIUS_CONTROL: f32 = 4.0;
pub(crate) const RADIUS_FIELD: f32 = 6.0;
const RADIUS_CARD: f32 = 8.0;
const RADIUS_DIALOG: f32 = 12.0;

pub(crate) const ICON_BUTTON: f32 = 28.0;
pub(crate) const CONTROL_PADDING: [f32; 2] = [6.0, SPACE_M];

pub(crate) const DANGER_ALPHA: f32 = 0.14;
pub(crate) const SOFT_ALPHA: [f32; 3] = [0.15, 0.22, 0.28];

/// Only `bg_base` is ever translucent (window blur); popups and dialogs are opaque `raised`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    pub(crate) bg_base: Color,
    pub(crate) raised: Color,
    pub(crate) raised_border: Color,
    pub(crate) text_primary: Color,
    pub(crate) text_secondary: Color,
    pub(crate) text_tertiary: Color,
    pub(crate) accent: Color,
    pub(crate) accent_text: Color,
    pub(crate) on_accent: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) error: Color,
    pub(crate) on_error: Color,
    pub(crate) hover: Color,
    pub(crate) pressed: Color,
}

impl Palette {
    pub(crate) const fn selected(&self) -> Color {
        selection(self.dark)
    }
}

impl std::hash::Hash for Palette {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark.hash(state);
        for color in [
            self.bg_base,
            self.raised,
            self.raised_border,
            self.text_primary,
            self.text_secondary,
            self.text_tertiary,
            self.accent,
            self.accent_text,
            self.on_accent,
            self.success,
            self.warning,
            self.error,
            self.on_error,
            self.hover,
            self.pressed,
        ] {
            for channel in [color.r, color.g, color.b, color.a] {
                state.write_u32(channel.to_bits());
            }
        }
    }
}

pub(crate) const fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The strongest overlay ever drawn under text; the theme contrast check relies on it.
pub(crate) const fn selection(dark: bool) -> Color {
    if dark {
        alpha(Color::WHITE, 0.08)
    } else {
        alpha(Color::BLACK, 0.06)
    }
}

pub(crate) fn rounded(color: Color, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

pub(crate) fn window(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(p.bg_base.into()),
        text_color: Some(p.text_primary),
        ..container::Style::default()
    }
}

fn raised_border(p: Palette, radius: f32) -> Border {
    Border {
        color: p.raised_border,
        width: 1.0,
        radius: radius.into(),
    }
}

const fn popup_shadow(p: Palette) -> Shadow {
    Shadow {
        color: alpha(Color::BLACK, if p.dark { 0.36 } else { 0.12 }),
        offset: Vector::new(0.0, 4.0),
        blur_radius: 12.0,
    }
}

fn raised(p: Palette, radius: f32, shadow: Shadow) -> container::Style {
    container::Style {
        background: Some(p.raised.into()),
        text_color: Some(p.text_primary),
        border: raised_border(p, radius),
        shadow,
        snap: true,
    }
}

pub(crate) fn popup(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| raised(p, RADIUS_CARD, popup_shadow(p))
}

pub(crate) fn dialog(p: Palette) -> impl Fn(&Theme) -> container::Style {
    let shadow = Shadow {
        color: alpha(Color::BLACK, if p.dark { 0.5 } else { 0.18 }),
        offset: Vector::new(0.0, 12.0),
        blur_radius: 32.0,
    };
    move |_| raised(p, RADIUS_DIALOG, shadow)
}

pub(crate) fn backdrop(p: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(p.bg_base, if p.dark { 0.6 } else { 0.5 }).into()),
        ..container::Style::default()
    }
}

pub(crate) fn row(p: Palette, selected: bool) -> impl Fn(&Theme) -> container::Style {
    rounded(
        if selected {
            p.selected()
        } else {
            Color::TRANSPARENT
        },
        RADIUS_FIELD,
    )
}
