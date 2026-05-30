use crate::style::Palette;
use iced::widget::container;
use iced::{Border, Color, Theme};

pub(crate) fn row_selected(p: &Palette) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| container::Style {
        background: Some(
            Color {
                a: 0.08,
                ..p.accent
            }
            .into(),
        ),
        border: Border {
            color: Color {
                a: 0.10,
                ..p.accent
            },
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}
