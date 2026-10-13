use super::{DANGER_ALPHA, Palette, RADIUS_CONTROL, SOFT_ALPHA, alpha};
use iced::widget::button::{Status, Style};
use iced::{Border, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Primary,
    Secondary,
    Ghost,
    Danger,
    MenuItem { highlighted: bool },
    MenuDanger,
    Nav { active: bool },
    Close,
}

pub(crate) fn button(p: Palette, kind: Button) -> impl Fn(&Theme, Status) -> Style {
    move |_, status| {
        let level = match status {
            Status::Pressed => 2,
            Status::Hovered => 1,
            Status::Active | Status::Disabled => 0,
        };
        let hot = level > 0;
        let overlay = [None, Some(p.hover), Some(p.pressed)][level];
        let menu_fill = [None, Some(p.selected()), Some(p.pressed)][level];
        let emphasis = if hot {
            p.text_primary
        } else {
            p.text_secondary
        };

        let (background, text_color) = match kind {
            Button::Primary => (Some(alpha(p.accent, SOFT_ALPHA[level])), p.accent_text),
            Button::Secondary => (Some([p.hover, p.pressed, p.pressed][level]), p.text_primary),
            Button::Ghost | Button::Nav { active: false } => (overlay, emphasis),
            Button::Danger => (
                Some(alpha(p.error, DANGER_ALPHA * [1.0, 1.5, 2.0][level])),
                p.error,
            ),
            Button::MenuItem { highlighted } => (
                menu_fill.or_else(|| highlighted.then_some(p.selected())),
                p.text_primary,
            ),
            Button::MenuDanger => (menu_fill, if hot { p.text_primary } else { p.error }),
            Button::Nav { active: true } => (Some(overlay.unwrap_or(p.hover)), p.text_primary),
            Button::Close if hot => (Some(p.error), p.on_error),
            Button::Close => (None, p.text_secondary),
        };

        let style = Style {
            background: background.map(Into::into),
            text_color,
            border: Border {
                radius: RADIUS_CONTROL.into(),
                ..Border::default()
            },
            ..Style::default()
        };
        if status == Status::Disabled {
            Style {
                background: style.background.map(|b| b.scale_alpha(0.5)),
                text_color: alpha(style.text_color, 0.5),
                ..style
            }
        } else {
            style
        }
    }
}
