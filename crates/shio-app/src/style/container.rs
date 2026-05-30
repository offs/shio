use crate::style::Palette;
use iced::widget::container;
use iced::{Border, Color, Shadow, Theme, Vector};
use shio_core::WindowMaterialPreference;

const DARK_LUMINANCE_LIMIT: f32 = 0.5;
const DARK_MODAL_ALPHA: f32 = 0.98;
const LIGHT_MODAL_ALPHA: f32 = 0.92;
const DARK_BACKDROP_ALPHA: f32 = 0.54;
const LIGHT_BACKDROP_ALPHA: f32 = 0.24;

pub(crate) fn titlebar(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.bg_surface.into()),
        border: Border {
            color: p.border_subtle,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn tab_bar(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.bg_base.into()),
        ..container::Style::default()
    }
}

pub(crate) fn card(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.bg_elevated.into()),
        border: Border {
            color: p.border_default,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color {
                a: 0.4,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        text_color: None,
        ..container::Style::default()
    }
}

pub(crate) fn modal_card(
    p: &Palette,
    material: WindowMaterialPreference,
) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| {
        let is_dark = is_dark_palette(&p);
        let background = Color {
            a: if material == WindowMaterialPreference::Solid {
                1.0
            } else if is_dark {
                DARK_MODAL_ALPHA
            } else {
                LIGHT_MODAL_ALPHA
            },
            ..p.bg_elevated
        };
        let border_color = Color {
            a: if is_dark { 0.12 } else { 0.20 },
            ..p.border_default
        };
        let shadow_alpha = if is_dark { 0.64 } else { 0.34 };

        container::Style {
            background: Some(background.into()),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: Shadow {
                color: Color {
                    a: shadow_alpha,
                    ..Color::BLACK
                },
                offset: Vector::new(0.0, 14.0),
                blur_radius: 36.0,
            },
            text_color: None,
            ..container::Style::default()
        }
    }
}

pub(crate) fn section(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.bg_surface.into()),
        border: Border {
            color: p.border_subtle,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn status_bar(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.bg_surface.into()),
        border: Border {
            color: p.border_subtle,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn download_row_container(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

pub(crate) fn modal_backdrop(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| {
        let alpha = if is_dark_palette(&p) {
            DARK_BACKDROP_ALPHA
        } else {
            LIGHT_BACKDROP_ALPHA
        };

        container::Style {
            background: Some(
                Color {
                    a: alpha,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..container::Style::default()
        }
    }
}

pub(crate) fn column_header(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        text_color: Some(p.text_ghost),
        ..container::Style::default()
    }
}

pub(crate) fn window_background(
    p: &Palette,
    material: WindowMaterialPreference,
) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(window_background_color(&p, material).into()),
        ..container::Style::default()
    }
}

const fn window_background_color(p: &Palette, material: WindowMaterialPreference) -> Color {
    match material {
        WindowMaterialPreference::Acrylic => p.bg_base,
        WindowMaterialPreference::Solid => Color {
            a: 1.0,
            ..p.bg_base
        },
    }
}

pub(crate) fn separator(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(p.border_subtle.into()),
        ..container::Style::default()
    }
}

fn is_dark_palette(p: &Palette) -> bool {
    let luminance = 0.0722f32.mul_add(
        p.bg_base.b,
        0.2126f32.mul_add(p.bg_base.r, 0.7152 * p.bg_base.g),
    );
    luminance < DARK_LUMINANCE_LIMIT
}
