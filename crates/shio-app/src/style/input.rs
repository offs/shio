use crate::style::Palette;
use iced::widget::{text_editor, text_input};
use iced::{Border, Color, Theme};

pub(crate) fn search_input(
    p: &Palette,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    let p = *p;
    move |_theme, _status| text_input::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        icon: p.text_tertiary,
        placeholder: p.text_tertiary,
        value: p.text_primary,
        selection: Color { a: 0.3, ..p.accent },
    }
}

pub(crate) fn input(p: &Palette) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    let p = *p;
    move |_theme, status| {
        let base = text_input::Style {
            background: p.bg_surface.into(),
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
            icon: p.text_tertiary,
            placeholder: p.text_tertiary,
            value: p.text_primary,
            selection: Color { a: 0.3, ..p.accent },
        };
        match status {
            text_input::Status::Active => base,
            text_input::Status::Hovered => text_input::Style {
                border: Border {
                    color: Color {
                        a: p.border_default.a * 1.5,
                        ..p.border_default
                    },
                    ..base.border
                },
                ..base
            },
            text_input::Status::Focused { .. } => text_input::Style {
                border: Border {
                    color: p.accent,
                    width: 1.5,
                    ..base.border
                },
                ..base
            },
            text_input::Status::Disabled => text_input::Style {
                background: Color {
                    a: 0.5,
                    ..p.bg_surface
                }
                .into(),
                value: p.text_ghost,
                ..base
            },
        }
    }
}

pub(crate) fn text_editor_style(
    p: &Palette,
) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    let p = *p;
    move |_theme, status| {
        let base = text_editor::Style {
            background: p.bg_surface.into(),
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
            placeholder: p.text_tertiary,
            value: p.text_primary,
            selection: Color { a: 0.3, ..p.accent },
        };
        match status {
            text_editor::Status::Active => base,
            text_editor::Status::Hovered => text_editor::Style {
                border: Border {
                    color: Color {
                        a: p.border_default.a * 1.5,
                        ..p.border_default
                    },
                    ..base.border
                },
                ..base
            },
            text_editor::Status::Focused { .. } => text_editor::Style {
                border: Border {
                    color: p.accent,
                    width: 1.5,
                    ..base.border
                },
                ..base
            },
            text_editor::Status::Disabled => text_editor::Style {
                background: Color {
                    a: 0.5,
                    ..p.bg_surface
                }
                .into(),
                value: p.text_ghost,
                ..base
            },
        }
    }
}
