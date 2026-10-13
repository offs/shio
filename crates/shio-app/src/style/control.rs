use super::{
    Palette, RADIUS_CARD, RADIUS_CONTROL, RADIUS_FIELD, alpha, popup_shadow, raised_border,
};
use iced::overlay::menu;
use iced::widget::{
    checkbox as checkbox_widget, container, pick_list as pick_list_widget, progress_bar,
    scrollable as scrollable_widget, slider as slider_widget, text_editor as text_editor_widget,
    text_input as text_input_widget, toggler as toggler_widget,
};
use iced::{Background, Border, Color, Theme};

fn field_border(p: Palette, focused: bool) -> Border {
    Border {
        color: if focused {
            alpha(p.accent, 0.55)
        } else {
            Color::TRANSPARENT
        },
        width: 1.0,
        radius: RADIUS_FIELD.into(),
    }
}

fn field_fill(p: Palette, hovered: bool) -> Background {
    if hovered { p.pressed } else { p.hover }.into()
}

pub(crate) fn text_input(
    p: Palette,
) -> impl Fn(&Theme, text_input_widget::Status) -> text_input_widget::Style {
    use text_input_widget::Status;
    move |_, status| text_input_widget::Style {
        background: field_fill(p, status == Status::Hovered),
        border: field_border(p, matches!(status, Status::Focused { .. })),
        icon: p.text_tertiary,
        placeholder: p.text_tertiary,
        value: if status == Status::Disabled {
            p.text_tertiary
        } else {
            p.text_primary
        },
        selection: alpha(p.accent, 0.35),
    }
}

pub(crate) fn text_editor(
    p: Palette,
) -> impl Fn(&Theme, text_editor_widget::Status) -> text_editor_widget::Style {
    use text_editor_widget::Status;
    move |_, status| text_editor_widget::Style {
        background: field_fill(p, status == Status::Hovered),
        border: field_border(p, matches!(status, Status::Focused { .. })),
        placeholder: p.text_tertiary,
        value: if status == Status::Disabled {
            p.text_tertiary
        } else {
            p.text_primary
        },
        selection: alpha(p.accent, 0.35),
    }
}

pub(crate) fn toggler(
    p: Palette,
) -> impl Fn(&Theme, toggler_widget::Status) -> toggler_widget::Style {
    use toggler_widget::Status;
    move |_, status| {
        let (on, disabled) = match status {
            Status::Active { is_toggled } | Status::Hovered { is_toggled } => (is_toggled, false),
            Status::Disabled { is_toggled } => (is_toggled, true),
        };
        let (track, knob) = if on {
            (p.accent, p.on_accent)
        } else {
            (p.pressed, p.text_secondary)
        };
        let fade = if disabled { 0.5 } else { 1.0 };
        toggler_widget::Style {
            background: track.scale_alpha(fade).into(),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            foreground: knob.scale_alpha(fade).into(),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(p.text_primary),
            border_radius: None,
            padding_ratio: 0.2,
        }
    }
}

pub(crate) fn checkbox(
    p: Palette,
) -> impl Fn(&Theme, checkbox_widget::Status) -> checkbox_widget::Style {
    use checkbox_widget::Status;
    move |_, status| {
        let (checked, hovered) = match status {
            Status::Active { is_checked } | Status::Disabled { is_checked } => (is_checked, false),
            Status::Hovered { is_checked } => (is_checked, true),
        };
        checkbox_widget::Style {
            background: if checked { p.accent } else { p.hover }.into(),
            icon_color: p.on_accent,
            border: Border {
                color: if checked {
                    p.accent
                } else if hovered {
                    p.text_tertiary
                } else {
                    alpha(p.text_tertiary, 0.5)
                },
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
            text_color: Some(p.text_primary),
        }
    }
}

pub(crate) fn slider(p: Palette) -> impl Fn(&Theme, slider_widget::Status) -> slider_widget::Style {
    use slider_widget::Status;
    move |_, status| slider_widget::Style {
        rail: slider_widget::Rail {
            backgrounds: (p.accent.into(), p.pressed.into()),
            width: 4.0,
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
        },
        handle: slider_widget::Handle {
            shape: slider_widget::HandleShape::Circle {
                radius: if status == Status::Active { 7.0 } else { 8.0 },
            },
            background: p.raised.into(),
            border_width: 1.5,
            border_color: p.accent,
        },
    }
}

pub(crate) fn pick_list(
    p: Palette,
) -> impl Fn(&Theme, pick_list_widget::Status) -> pick_list_widget::Style {
    use pick_list_widget::Status;
    move |_, status| pick_list_widget::Style {
        text_color: p.text_primary,
        placeholder_color: p.text_tertiary,
        handle_color: p.text_tertiary,
        background: field_fill(p, status == Status::Hovered),
        border: field_border(p, matches!(status, Status::Opened { .. })),
    }
}

pub(crate) fn pick_list_menu(p: Palette) -> impl Fn(&Theme) -> menu::Style {
    move |_| menu::Style {
        background: p.raised.into(),
        border: raised_border(p, RADIUS_CARD),
        text_color: p.text_primary,
        selected_text_color: p.text_primary,
        selected_background: p.selected().into(),
        shadow: popup_shadow(p),
    }
}

pub(crate) fn scrollable(
    p: Palette,
) -> impl Fn(&Theme, scrollable_widget::Status) -> scrollable_widget::Style {
    use scrollable_widget::Status;
    move |_, status| {
        let rail = |active: bool| scrollable_widget::Rail {
            background: None,
            border: Border::default(),
            scroller: scrollable_widget::Scroller {
                background: if active { p.text_tertiary } else { p.pressed }.into(),
                border: Border {
                    radius: 4.0.into(),
                    ..Border::default()
                },
            },
        };
        let (vertical, horizontal) = match status {
            Status::Active { .. } => (false, false),
            Status::Hovered {
                is_vertical_scrollbar_hovered,
                is_horizontal_scrollbar_hovered,
                ..
            } => (
                is_vertical_scrollbar_hovered,
                is_horizontal_scrollbar_hovered,
            ),
            Status::Dragged {
                is_vertical_scrollbar_dragged,
                is_horizontal_scrollbar_dragged,
                ..
            } => (
                is_vertical_scrollbar_dragged,
                is_horizontal_scrollbar_dragged,
            ),
        };
        scrollable_widget::Style {
            container: container::Style::default(),
            vertical_rail: rail(vertical),
            horizontal_rail: rail(horizontal),
            gap: None,
            auto_scroll: scrollable_widget::AutoScroll {
                background: p.raised.into(),
                border: raised_border(p, RADIUS_CARD),
                shadow: popup_shadow(p),
                icon: p.text_secondary,
            },
        }
    }
}

pub(crate) fn progress(p: Palette, bar: Color) -> impl Fn(&Theme) -> progress_bar::Style {
    move |_| progress_bar::Style {
        background: p.hover.into(),
        bar: bar.into(),
        border: Border {
            radius: 1.0.into(),
            ..Border::default()
        },
    }
}
