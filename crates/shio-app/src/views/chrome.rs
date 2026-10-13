use super::SEARCH_ID;
use crate::message::Message;
use crate::style::{self, Button, ICON_BUTTON, Palette, SPACE_L, SPACE_M, SPACE_S, SPACE_XS};
use crate::widgets::{icon_button, search_field};
use iced::widget::text::Wrapping;
use iced::widget::text_input::{Icon, Side};
use iced::widget::{
    Space, Text, button, center, column, container, mouse_area, row, stack, text, text_input,
};
use iced::window::Direction;
use iced::{Alignment, Element, Length, Padding, Pixels, mouse};
use iced_fonts::bootstrap;

pub(super) const TITLEBAR_HEIGHT: f32 = 38.0;
const INSET: [f32; 2] = [SPACE_L, 5.0];
const RESIZE_EDGE: f32 = 6.0;
const SEARCH_PADDING: [f32; 2] = [5.0, 10.0];
// `bootstrap::search()`, as the char a text input icon needs.
const SEARCH_GLYPH: char = '\u{f52a}';
// The search box stays centred between equal sides this wide; narrow windows shrink the box.
const SIDES_WIDTH: f32 = if cfg!(target_os = "macos") {
    300.0
} else {
    440.0
};

#[derive(Clone, Copy)]
pub(crate) enum Search<'a> {
    Hidden,
    /// Drawn while a modal is open, so it can never take focus.
    Inert(&'a str),
    Live {
        query: &'a str,
        focused: bool,
        suggesting: bool,
    },
}

/// The search field's left edge and width.
pub(super) fn search_span(window_width: f32) -> (f32, f32) {
    let width = (window_width - SIDES_WIDTH).clamp(160.0, 320.0);
    let sides = window_width - INSET[0] - INSET[1] - width;
    (INSET[0] + sides / 2.0, width)
}

pub(crate) fn titlebar<'a>(
    search: Search<'a>,
    maximized: bool,
    window_width: f32,
    p: &Palette,
) -> Element<'a, Message> {
    let (_, width) = search_span(window_width);
    let has_list = !matches!(search, Search::Hidden);
    let brand: Element<'a, Message> = if cfg!(target_os = "macos") {
        Space::new().width(72).into()
    } else {
        text("shio")
            .size(style::TEXT_BODY)
            .color(p.text_secondary)
            .into()
    };
    let add = has_list.then(|| {
        button(
            row![
                bootstrap::plus_lg().size(11),
                text("add").size(style::TEXT_SMALL)
            ]
            .spacing(SPACE_XS)
            .align_y(Alignment::Center),
        )
        .padding([4.0, 10.0])
        .style(style::button(*p, Button::Primary))
        .on_press(Message::OpenAdd)
    });
    let leading = row![brand]
        .push(add)
        .spacing(SPACE_M)
        .align_y(Alignment::Center);

    let centre: Element<'a, Message> = match search {
        Search::Hidden => Space::new().into(),
        Search::Inert(query) => container(
            row![
                bootstrap::search().size(11).color(p.text_tertiary),
                text(if query.is_empty() { "search" } else { query })
                    .size(style::TEXT_SMALL)
                    .color(if query.is_empty() {
                        p.text_tertiary
                    } else {
                        p.text_primary
                    })
                    .wrapping(Wrapping::None),
            ]
            .spacing(SPACE_S)
            .align_y(Alignment::Center),
        )
        .padding(SEARCH_PADDING)
        .width(width)
        .clip(true)
        .style(style::rounded(p.hover, style::RADIUS_FIELD))
        .into(),
        Search::Live {
            query,
            focused,
            suggesting,
        } => search_field(
            text_input("search", query)
                .id(SEARCH_ID)
                .on_input(Message::SearchChanged)
                .icon(Icon {
                    font: iced_fonts::BOOTSTRAP_FONT,
                    code_point: SEARCH_GLYPH,
                    size: Some(Pixels(11.0)),
                    spacing: SPACE_S,
                    side: Side::Left,
                })
                .size(style::TEXT_SMALL)
                .padding(SEARCH_PADDING)
                .width(width)
                .style(style::text_input(*p)),
            focused,
            suggesting,
        ),
    };

    let trailing = row![]
        .push(has_list.then(|| {
            icon_button(
                bootstrap::gear(),
                "settings",
                Some(Message::OpenSettings),
                p,
            )
        }))
        .push((!cfg!(target_os = "macos")).then(|| window_controls(maximized, p)))
        .spacing(SPACE_M)
        .align_y(Alignment::Center);

    let bar = row![
        container(leading).width(Length::Fill),
        centre,
        container(trailing).align_right(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .height(TITLEBAR_HEIGHT)
    .padding(Padding::ZERO.left(INSET[0]).right(INSET[1]));

    mouse_area(bar)
        .on_press(Message::WindowDrag)
        .on_double_click(Message::WindowMaximizeToggle)
        .into()
}

fn window_controls<'a>(maximized: bool, p: &Palette) -> Element<'a, Message> {
    let control = |icon: Text<'a>, message: Message, kind: Button| {
        button(center(icon.size(12)))
            .width(32)
            .height(ICON_BUTTON)
            .padding(0)
            .style(style::button(*p, kind))
            .on_press(message)
    };
    row![
        control(bootstrap::dash_lg(), Message::WindowMinimize, Button::Ghost),
        control(
            if maximized {
                bootstrap::back()
            } else {
                bootstrap::square()
            },
            Message::WindowMaximizeToggle,
            Button::Ghost,
        ),
        control(bootstrap::x_lg(), Message::WindowClose, Button::Close),
    ]
    .spacing(2)
    .into()
}

/// Resize grips for the undecorated window; `content` stays the stack's first child either way
/// so its widget state survives maximizing.
pub(crate) fn resizable<'a>(
    content: impl Into<Element<'a, Message>>,
    maximized: bool,
) -> Element<'a, Message> {
    if cfg!(target_os = "macos") {
        return content.into();
    }
    if maximized {
        return stack![content.into()].into();
    }
    let grip = |direction: Direction, width: Length, height: Length| {
        let cursor = match direction {
            Direction::North | Direction::South => mouse::Interaction::ResizingVertically,
            Direction::East | Direction::West => mouse::Interaction::ResizingHorizontally,
            Direction::NorthWest | Direction::SouthEast => {
                mouse::Interaction::ResizingDiagonallyDown
            },
            Direction::NorthEast | Direction::SouthWest => mouse::Interaction::ResizingDiagonallyUp,
        };
        mouse_area(Space::new().width(width).height(height))
            .interaction(cursor)
            .on_press(Message::WindowResize(direction))
    };
    let edge = Length::Fixed(RESIZE_EDGE);
    let side = |left: Direction, middle: Direction, right: Direction| {
        row![
            grip(left, edge, edge),
            grip(middle, Length::Fill, edge),
            grip(right, edge, edge),
        ]
    };
    let grips = column![
        side(Direction::NorthWest, Direction::North, Direction::NorthEast),
        row![
            grip(Direction::West, edge, Length::Fill),
            Space::new().width(Length::Fill),
            grip(Direction::East, edge, Length::Fill),
        ]
        .height(Length::Fill),
        side(Direction::SouthWest, Direction::South, Direction::SouthEast),
    ];
    stack![content.into(), grips].into()
}
