use crate::style::Palette;
use iced::widget::container;
use iced::{Border, Color, Shadow, Theme, Vector};

fn make_toast(p: &Palette, accent: Color) -> container::Style {
    container::Style {
        background: Some(p.bg_elevated.into()),
        border: Border {
            color: Color { a: 0.20, ..accent },
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: Shadow {
            color: Color {
                a: 0.3,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        },
        ..container::Style::default()
    }
}

pub(crate) fn toast(p: &Palette, accent: Color) -> impl Fn(&Theme) -> container::Style {
    let p = *p;
    move |_theme| make_toast(&p, accent)
}
