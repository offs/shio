use crate::message::{FirstRunStep, Message};
use crate::style;
use crate::theme::{ThemeCatalog, ThemeId};
use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, pick_list, row, scrollable,
    stack, text, toggler,
};
use iced::{Border, Element, Length, Padding, Theme};
use shio_core::{AppConfig, WindowMaterialPreference};

const MODAL_WIDTH: f32 = 620.0;
const MODAL_HEIGHT: f32 = 392.0;
const RAIL_WIDTH: f32 = 124.0;
const CONTROL_WIDTH: f32 = 248.0;
const PATH_ROW_WIDTH: f32 = 340.0;

pub(crate) fn view<'a>(
    config: &'a AppConfig,
    step: FirstRunStep,
    theme_catalog: &'a ThemeCatalog,
    p: &'a style::Palette,
    material: WindowMaterialPreference,
    base: Element<'a, Message>,
) -> Element<'a, Message> {
    let frame = column![
        header(p),
        body(config, step, theme_catalog, p),
        footer(step, p),
    ]
    .height(Length::Fill);

    let card = container(frame)
        .width(MODAL_WIDTH)
        .height(MODAL_HEIGHT)
        .style(style::modal_card(p, material));

    let overlay = mouse_area(center(opaque(card)));

    let backdrop = container(overlay)
        .style(style::modal_backdrop(p))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop].into()
}

fn header(p: &style::Palette) -> Element<'_, Message> {
    container(text("set up shio").size(20).color(p.text_primary))
        .padding(Padding::default().top(24).right(28).bottom(8).left(28))
        .width(Length::Fill)
        .into()
}

fn body<'a>(
    config: &'a AppConfig,
    step: FirstRunStep,
    theme_catalog: &'a ThemeCatalog,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let content = match step {
        FirstRunStep::Look => look_step(config, theme_catalog, p),
        FirstRunStep::Folder => folder_step(config, p),
        FirstRunStep::Behavior => behavior_step(config, p),
    };

    let pane = scrollable(container(content).padding(Padding::default().top(8).right(28).left(28)))
        .height(Length::Fill)
        .style(style::scrollable_style(p));

    container(row![rail(step, p), divider(p), pane].height(Length::Fill))
        .height(Length::Fill)
        .into()
}

fn rail(step: FirstRunStep, p: &style::Palette) -> Element<'_, Message> {
    container(
        column![
            rail_item("look", step, FirstRunStep::Look, p),
            rail_item("folder", step, FirstRunStep::Folder, p),
            rail_item("behavior", step, FirstRunStep::Behavior, p),
        ]
        .spacing(4),
    )
    .padding(Padding::default().top(8).right(10).left(18))
    .width(RAIL_WIDTH)
    .height(Length::Fill)
    .into()
}

fn rail_item<'a>(
    label: &'a str,
    current: FirstRunStep,
    item: FirstRunStep,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let active = current == item;
    let complete = step_index(item) < step_index(current);
    let text_color = if active {
        p.text_primary
    } else if complete {
        p.text_secondary
    } else {
        p.text_tertiary
    };
    let bg = if active {
        Some(
            iced::Color {
                a: 0.14,
                ..p.accent
            }
            .into(),
        )
    } else {
        None
    };
    let border = Border {
        radius: 4.0.into(),
        ..Border::default()
    };

    container(text(label).size(13).color(text_color))
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: bg,
            border,
            ..container::Style::default()
        })
        .into()
}

fn divider(p: &style::Palette) -> Element<'_, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(style::separator(p))
        .width(1)
        .height(Length::Fill)
        .into()
}

fn look_step<'a>(
    config: &'a AppConfig,
    theme_catalog: &'a ThemeCatalog,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    column![
        section_title("look", "choose the interface colors.", p),
        Space::new().height(16),
        theme_picker_row(config, theme_catalog, p),
        Space::new().height(8),
        material_picker_row(config.window.material, p),
        Space::new().height(18),
        theme_preview(p),
    ]
    .into()
}

fn folder_step<'a>(config: &'a AppConfig, p: &'a style::Palette) -> Element<'a, Message> {
    column![
        section_title(
            "folder",
            "choose where new downloads are saved by default.",
            p,
        ),
        Space::new().height(18),
        download_dir_row(config.download_dir.to_string_lossy().into_owned(), p),
    ]
    .into()
}

fn behavior_step<'a>(config: &'a AppConfig, p: &'a style::Palette) -> Element<'a, Message> {
    column![
        section_title("behavior", "choose the system behaviors you want.", p),
        Space::new().height(12),
        file_associations_row(p),
        divider_thin(p),
        toggle_row(
            "clipboard monitoring",
            "detect copied urls",
            config.clipboard_monitor,
            Message::ToggleClipboard,
            p,
        ),
        divider_thin(p),
        toggle_row(
            "desktop notifications",
            "notify when downloads finish",
            config.notifications,
            Message::ToggleNotifications,
            p,
        ),
        divider_thin(p),
        toggle_row(
            "close to tray",
            "keep running when the window closes",
            config.close_to_tray,
            Message::CloseToTrayToggled,
            p,
        ),
    ]
    .into()
}

fn section_title<'a>(
    title: &'a str,
    description: &'a str,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    column![
        text(title).size(15).color(p.text_primary),
        Space::new().height(5),
        text(description).size(12).color(p.text_tertiary),
    ]
    .into()
}

fn theme_picker_row<'a>(
    config: &'a AppConfig,
    catalog: &'a ThemeCatalog,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let picker = pick_list(
        catalog.ids(),
        ThemeId::parse(&config.theme.id).ok(),
        Message::ThemeChanged,
    )
    .text_size(13)
    .padding([7, 12])
    .width(Length::Fixed(CONTROL_WIDTH))
    .style(style::pick_list_style(p))
    .menu_style(style::menu_style(p));

    compact_labeled_row("theme", picker.into(), p)
}

fn material_picker_row(
    current: WindowMaterialPreference,
    p: &style::Palette,
) -> Element<'_, Message> {
    let picker = pick_list(
        &WindowMaterialPreference::ALL[..],
        Some(current),
        Message::ThemeMaterialChanged,
    )
    .text_size(13)
    .padding([7, 12])
    .width(Length::Fixed(CONTROL_WIDTH))
    .style(style::pick_list_style(p))
    .menu_style(style::menu_style(p));

    compact_labeled_row("window material", picker.into(), p)
}

fn compact_labeled_row<'a>(
    label: &'a str,
    control: Element<'a, Message>,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    row![
        text(label)
            .size(13)
            .color(p.text_primary)
            .width(Length::Fill),
        control,
    ]
    .height(Length::Fixed(42.0))
    .align_y(iced::Alignment::Center)
    .into()
}

fn theme_preview(p: &style::Palette) -> Element<'_, Message> {
    container(
        row![
            preview_swatch("base", p.bg_base, p),
            preview_swatch("surface", p.bg_elevated, p),
            preview_swatch("accent", p.accent, p),
        ]
        .spacing(8),
    )
    .padding(8)
    .style(style::section(p))
    .width(Length::Fill)
    .into()
}

fn preview_swatch<'a>(
    label: &'a str,
    color: iced::Color,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let swatch = container(Space::new().height(18))
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            border: Border {
                color: p.border_default,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .width(Length::Fill);

    column![
        swatch,
        Space::new().height(5),
        text(label).size(11).color(p.text_tertiary),
    ]
    .width(Length::Fill)
    .into()
}

fn download_dir_row(current: String, p: &style::Palette) -> Element<'_, Message> {
    let path_display = container(
        row![
            iced_fonts::bootstrap::folder()
                .size(13)
                .color(p.text_tertiary),
            Space::new().width(8),
            text(current)
                .size(12)
                .color(p.text_secondary)
                .wrapping(iced::widget::text::Wrapping::None)
                .width(Length::Fill),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(style::section(p))
    .padding([8, 12])
    .width(Length::Fixed(PATH_ROW_WIDTH));

    let browse = button(text("change").size(12))
        .style(style::btn_secondary(p))
        .on_press(Message::PickSaveFolder)
        .padding([8, 14]);

    row![path_display, Space::new().width(8), browse]
        .align_y(iced::Alignment::Center)
        .into()
}

fn file_associations_row(p: &style::Palette) -> Element<'_, Message> {
    let setup = button(text("set up").size(12))
        .style(style::btn_secondary(p))
        .on_press(Message::SetUpFileAssociations)
        .padding([7, 14]);

    action_row(
        "file associations",
        "open .torrent files and magnet links with shio",
        setup.into(),
        p,
    )
}

fn toggle_row<'a>(
    title: &'a str,
    description: &'a str,
    value: bool,
    on_toggle: Message,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let control = toggler(value)
        .on_toggle(move |_| on_toggle.clone())
        .size(18)
        .style(style::toggler_style(p));

    action_row(title, description, control.into(), p)
}

fn action_row<'a>(
    title: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
    p: &'a style::Palette,
) -> Element<'a, Message> {
    let left = column![
        text(title).size(13).color(p.text_primary),
        Space::new().height(3),
        text(description).size(12).color(p.text_tertiary),
    ]
    .width(Length::Fill);

    row![left, Space::new().width(16), control]
        .align_y(iced::Alignment::Center)
        .padding([9, 0])
        .into()
}

fn divider_thin(p: &style::Palette) -> Element<'_, Message> {
    container(Space::new().height(1))
        .style(style::separator(p))
        .width(Length::Fill)
        .height(1)
        .into()
}

fn footer(step: FirstRunStep, p: &style::Palette) -> Element<'_, Message> {
    let back: Element<'_, Message> = if step.previous().is_some() {
        button(text("back").size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::FirstRunBack)
            .padding([8, 12])
            .into()
    } else {
        Space::new().width(58).height(32).into()
    };

    let skip: Element<'_, Message> = if step == FirstRunStep::Behavior {
        Space::new().width(0).height(32).into()
    } else {
        button(text("skip setup").size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::FirstRunSkip)
            .padding([8, 12])
            .into()
    };

    let next = button(text(primary_label(step)).size(13))
        .style(style::btn_primary(p))
        .on_press(Message::FirstRunNext)
        .padding([9, 18]);

    container(
        row![
            back,
            Space::new().width(Length::Fill),
            skip,
            Space::new().width(8),
            next,
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding::default().top(8).right(28).bottom(20).left(28))
    .width(Length::Fill)
    .into()
}

const fn step_index(step: FirstRunStep) -> u8 {
    match step {
        FirstRunStep::Look => 0,
        FirstRunStep::Folder => 1,
        FirstRunStep::Behavior => 2,
    }
}

const fn primary_label(step: FirstRunStep) -> &'static str {
    match step {
        FirstRunStep::Look => "continue to folder",
        FirstRunStep::Folder => "continue to behavior",
        FirstRunStep::Behavior => "start using shio",
    }
}
