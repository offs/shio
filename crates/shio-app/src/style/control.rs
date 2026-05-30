use crate::style::Palette;
use iced::overlay::menu;
use iced::widget::{container, pick_list, scrollable, slider, toggler};
use iced::{Border, Color, Shadow, Theme, Vector};

pub(crate) fn pick_list_style(
    p: &Palette,
) -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style {
    let p = *p;
    move |_theme, status| {
        let base = pick_list::Style {
            text_color: p.text_primary,
            placeholder_color: p.text_tertiary,
            handle_color: p.text_tertiary,
            background: p.bg_elevated.into(),
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
        };
        match status {
            pick_list::Status::Active => base,
            pick_list::Status::Hovered => pick_list::Style {
                border: Border {
                    color: Color {
                        a: p.border_default.a * 1.5,
                        ..p.border_default
                    },
                    ..base.border
                },
                ..base
            },
            pick_list::Status::Opened { .. } => pick_list::Style {
                border: Border {
                    color: p.accent,
                    ..base.border
                },
                ..base
            },
        }
    }
}

pub(crate) fn menu_style(p: &Palette) -> impl Fn(&Theme) -> menu::Style {
    let p = *p;
    move |_theme| menu::Style {
        background: p.bg_elevated.into(),
        border: Border {
            color: p.border_default,
            width: 1.0,
            radius: 4.0.into(),
        },
        text_color: p.text_primary,
        selected_text_color: p.text_primary,
        selected_background: p.bg_hover.into(),
        shadow: Shadow {
            color: Color {
                a: 0.3,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
    }
}

pub(crate) fn slider_style(p: &Palette) -> impl Fn(&Theme, slider::Status) -> slider::Style {
    let p = *p;
    move |_theme, status| {
        let rail = slider::Rail {
            backgrounds: (
                Color {
                    a: p.scroller_idle.a * 1.2,
                    ..p.scroller_idle
                }
                .into(),
                p.overlay_subtle.into(),
            ),
            width: 3.0,
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
        };
        let handle = slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.0 },
            background: p.text_secondary.into(),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        };
        match status {
            slider::Status::Active => slider::Style { rail, handle },
            slider::Status::Hovered => slider::Style {
                handle: slider::Handle {
                    background: p.text_primary.into(),
                    ..handle
                },
                rail,
            },
            slider::Status::Dragged => slider::Style {
                handle: slider::Handle {
                    background: p.accent.into(),
                    ..handle
                },
                rail,
            },
        }
    }
}

pub(crate) fn scrollable_style(
    p: &Palette,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    let p = *p;
    move |_theme, status| {
        let rail_with = |scroller_color: Color| scrollable::Rail {
            background: None,
            border: Border {
                radius: 3.0.into(),
                ..Border::default()
            },
            scroller: scrollable::Scroller {
                background: scroller_color.into(),
                border: Border {
                    radius: 3.0.into(),
                    ..Border::default()
                },
            },
        };

        let (vertical, horizontal) = match status {
            scrollable::Status::Active { .. } => {
                (rail_with(p.scroller_idle), rail_with(p.scroller_idle))
            },
            scrollable::Status::Hovered {
                is_vertical_scrollbar_hovered,
                is_horizontal_scrollbar_hovered,
                ..
            } => (
                rail_with(if is_vertical_scrollbar_hovered {
                    p.scroller_hovered
                } else {
                    p.scroller_idle
                }),
                rail_with(if is_horizontal_scrollbar_hovered {
                    p.scroller_hovered
                } else {
                    p.scroller_idle
                }),
            ),
            scrollable::Status::Dragged {
                is_vertical_scrollbar_dragged,
                is_horizontal_scrollbar_dragged,
                ..
            } => (
                rail_with(if is_vertical_scrollbar_dragged {
                    p.scroller_dragged
                } else {
                    p.scroller_idle
                }),
                rail_with(if is_horizontal_scrollbar_dragged {
                    p.scroller_dragged
                } else {
                    p.scroller_idle
                }),
            ),
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: vertical,
            horizontal_rail: horizontal,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Color {
                    a: 0.9,
                    ..p.bg_elevated
                }
                .into(),
                border: Border {
                    color: p.border_default,
                    width: 1.0,
                    radius: f32::MAX.into(),
                },
                shadow: Shadow {
                    color: Color {
                        a: 0.7,
                        ..Color::BLACK
                    },
                    offset: Vector::ZERO,
                    blur_radius: 2.0,
                },
                icon: p.text_secondary,
            },
        }
    }
}

pub(crate) fn toggler_style(p: &Palette) -> impl Fn(&Theme, toggler::Status) -> toggler::Style {
    let p = *p;
    move |_theme, status| {
        let (bg, fg) = match status {
            toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
                if is_toggled {
                    (p.accent, Color::WHITE)
                } else {
                    (p.toggler_off_bg, p.toggler_off_fg)
                }
            },
            toggler::Status::Disabled { .. } => (
                Color {
                    a: p.toggler_off_bg.a * 0.5,
                    ..p.toggler_off_bg
                },
                p.text_ghost,
            ),
        };
        toggler::Style {
            background: bg.into(),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            foreground: fg.into(),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(p.text_secondary),
            border_radius: None,
            padding_ratio: 0.2,
        }
    }
}
