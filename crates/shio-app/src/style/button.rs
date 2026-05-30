use crate::style::Palette;
use iced::widget::button;
use iced::{Border, Color, Shadow, Theme};

pub(crate) fn btn_primary(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: Some(p.bg_elevated.into()),
            text_color: p.text_primary,
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style {
                background: Some(p.bg_hover.into()),
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(p.bg_active.into()),
                ..base
            },
            button::Status::Disabled => button::Style {
                text_color: p.text_ghost,
                border: Border {
                    color: Color {
                        a: 0.04,
                        ..p.border_default
                    },
                    ..base.border
                },
                ..base
            },
        }
    }
}

pub(crate) fn btn_secondary(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: None,
            text_color: p.text_secondary,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style {
                background: Some(p.bg_hover.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(p.bg_active.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Disabled => button::Style {
                text_color: p.text_ghost,
                ..base
            },
        }
    }
}

pub(crate) fn btn_ghost(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: None,
            text_color: p.text_secondary,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style {
                background: Some(p.overlay_hover.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(
                    Color {
                        a: p.overlay_hover.a * 1.6,
                        ..p.overlay_hover
                    }
                    .into(),
                ),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Disabled => button::Style {
                text_color: p.text_ghost,
                ..base
            },
        }
    }
}

pub(crate) fn sidebar_item(
    p: &Palette,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: None,
            text_color: if active {
                p.text_primary
            } else {
                p.text_secondary
            },
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        if active {
            return button::Style {
                background: Some(
                    Color {
                        a: 0.14,
                        ..p.accent
                    }
                    .into(),
                ),
                text_color: p.text_primary,
                ..base
            };
        }
        match status {
            button::Status::Hovered => button::Style {
                background: Some(p.bg_hover.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(p.bg_active.into()),
                text_color: p.text_primary,
                ..base
            },
            _ => base,
        }
    }
}

pub(crate) fn btn_icon(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: None,
            text_color: p.text_tertiary,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Hovered => button::Style {
                background: Some(p.overlay_hover.into()),
                text_color: p.text_secondary,
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(
                    Color {
                        a: p.overlay_hover.a * 1.6,
                        ..p.overlay_hover
                    }
                    .into(),
                ),
                text_color: p.text_secondary,
                ..base
            },
            button::Status::Active | button::Status::Disabled => base,
        }
    }
}

pub(crate) fn btn_danger(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: Some(p.error.into()),
            text_color: p.text_primary,
            border: Border {
                color: p.error,
                width: 1.0,
                radius: 4.0.into(),
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style {
                background: Some(Color { a: 0.88, ..p.error }.into()),
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(Color { a: 0.76, ..p.error }.into()),
                ..base
            },
            button::Status::Disabled => button::Style {
                background: Some(Color { a: 0.3, ..p.error }.into()),
                text_color: p.text_ghost,
                border: Border {
                    color: Color { a: 0.3, ..p.error },
                    ..base.border
                },
                ..base
            },
        }
    }
}

pub(crate) fn btn_dropdown(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        let base = button::Style {
            background: None,
            text_color: p.text_secondary,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            ..button::Style::default()
        };
        match status {
            button::Status::Hovered => button::Style {
                background: Some(p.bg_hover.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(p.bg_active.into()),
                text_color: p.text_primary,
                ..base
            },
            button::Status::Active | button::Status::Disabled => base,
        }
    }
}

pub(crate) fn btn_dropdown_active(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, _status| button::Style {
        background: Some(
            Color {
                a: 0.14,
                ..p.accent
            }
            .into(),
        ),
        text_color: p.text_primary,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        ..button::Style::default()
    }
}

pub(crate) fn tab(p: &Palette, active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    let p = *p;
    move |_theme, status| {
        if active {
            button::Style {
                background: None,
                text_color: p.text_primary,
                border: Border {
                    color: p.accent,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..button::Style::default()
            }
        } else {
            button::Style {
                background: match status {
                    button::Status::Hovered => Some(p.bg_hover.into()),
                    _ => None,
                },
                text_color: match status {
                    button::Status::Hovered => p.text_secondary,
                    _ => p.text_tertiary,
                },
                border: Border {
                    radius: 0.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        }
    }
}
