use crate::app::DragHover;
use crate::message::{DownloadColumn, DownloadColumnWidths, Message, SortCol, SortDirection};
use crate::style;
use crate::widgets::download_row;
use iced::widget::{
    Space, button, column, container, keyed_column, mouse_area, row, scrollable, text,
};
use iced::{Element, Length};
use shio_core::{Download, DownloadId};
use std::collections::HashSet;

pub(crate) fn row_widget_id(id: DownloadId) -> iced::widget::Id {
    iced::widget::Id::from(format!("dl-{}", id.0))
}

#[derive(Clone, Copy)]
pub(crate) struct ViewOptions<'a> {
    pub(crate) sort_col: SortCol,
    pub(crate) sort_dir: SortDirection,
    pub(crate) has_search: bool,
    pub(crate) drag: Option<&'a DragHover>,
    pub(crate) widths: DownloadColumnWidths,
    pub(crate) available_width: Option<f32>,
    pub(crate) scroll_long_names: bool,
    pub(crate) carousel_offset: usize,
}

pub(crate) fn view<'a>(
    downloads: &[(&'a Download, Option<Vec<u32>>)],
    selection: &HashSet<DownloadId>,
    options: ViewOptions<'_>,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let _span = tracing::trace_span!("download_list_view", rows = downloads.len()).entered();
    if downloads.is_empty() {
        return empty_state(p);
    }

    let widths = effective_column_widths(options.widths, options.available_width);
    let headers = column_headers(options.sort_col, options.sort_dir, widths, p);

    let drop_target = options.drag.and_then(|d| d.target_id);
    let drag_source = options.drag.map(|d| d.source_id);
    let mut rows: Vec<(DownloadId, Element<'a, Message>)> = Vec::with_capacity(downloads.len());

    for (dl, indices) in downloads {
        let is_selected = selection.contains(&dl.id);
        let drop_side = if drop_target == Some(dl.id) && drag_source != Some(dl.id) {
            options.drag.map(|d| d.side)
        } else {
            None
        };
        let highlight = if options.has_search {
            indices.as_deref()
        } else {
            None
        };
        let row_el = download_row::view(
            dl,
            is_selected,
            highlight,
            drop_side,
            download_row::NameDisplayOptions {
                carousel: options.scroll_long_names,
                carousel_offset: options.carousel_offset,
                name_width: widths.name,
            },
            widths,
            p,
        );

        let dl_id = dl.id;
        let widget_id = row_widget_id(dl.id);
        let draggable: Element<'a, Message> = iced_drop::droppable(row_el)
            .id(widget_id)
            .on_click(Message::SelectClicked(dl_id))
            .on_drop(move |_point, _rect| Message::DragDrop(dl_id))
            .on_drag(move |point, _rect| Message::DragUpdate(dl_id, point))
            .drag_overlay(true)
            .drag_hide(true)
            .drag_center(false)
            .into();

        rows.push((dl.id, draggable));
    }

    let rows = keyed_column(rows).spacing(2);

    column![headers, scrollable(rows).height(Length::Fill)]
        .width(Length::Fill)
        .into()
}

fn column_headers<'a>(
    sort_col: SortCol,
    sort_dir: SortDirection,
    widths: DownloadColumnWidths,
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let arrow = match sort_dir {
        SortDirection::Ascending => " \u{2191}",
        SortDirection::Descending => " \u{2193}",
    };

    let fixed_header = |label: &str, col: SortCol, width: f32| -> Element<'a, Message> {
        let label_text = if sort_col == col {
            format!("{label}{arrow}")
        } else {
            label.to_string()
        };
        button(text(label_text).size(10).color(p.text_ghost))
            .style(style::btn_ghost(p))
            .on_press(Message::SortColumn(col))
            .padding([4, 0])
            .width(Length::Fixed(width))
            .into()
    };

    let row = iced::widget::row![
        Space::new().width(28),
        fixed_header("name", SortCol::Name, widths.name),
        resize_handle(DownloadColumn::Name, p),
        fixed_header("size", SortCol::Size, widths.size),
        resize_handle(DownloadColumn::Size, p),
        fixed_header("progress", SortCol::Progress, widths.progress),
        resize_handle(DownloadColumn::Progress, p),
        fixed_header("speed", SortCol::Speed, widths.speed),
        resize_handle(DownloadColumn::Speed, p),
        fixed_header("eta", SortCol::Eta, widths.eta),
        resize_handle(DownloadColumn::Eta, p),
        Space::new().width(download_row::ACTION_COLUMN_WIDTH),
    ]
    .spacing(4)
    .padding([0, 16])
    .height(28)
    .width(Length::Fill);

    container(row)
        .style(style::column_header(p))
        .width(Length::Fill)
        .into()
}

fn effective_column_widths(
    widths: DownloadColumnWidths,
    available_width: Option<f32>,
) -> DownloadColumnWidths {
    let Some(available_width) = available_width else {
        return widths;
    };

    let fixed_width =
        COLUMN_LAYOUT_CHROME_WIDTH + widths.size + widths.progress + widths.speed + widths.eta;
    let name = widths.name.min((available_width - fixed_width).max(0.0));

    DownloadColumnWidths { name, ..widths }
}

const COLUMN_LAYOUT_CHROME_WIDTH: f32 =
    16.0 * 2.0 + 28.0 + 8.0 * 5.0 + download_row::ACTION_COLUMN_WIDTH + 4.0 * 11.0;

fn resize_handle(column: DownloadColumn, p: &crate::style::Palette) -> Element<'_, Message> {
    let line = container(Space::new().width(1).height(Length::Fill)).style(style::separator(p));
    mouse_area(container(line).width(8).height(Length::Fill).center_x(8))
        .on_press(Message::ColumnResizeStart(column))
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}

fn empty_state(p: &crate::style::Palette) -> Element<'_, Message> {
    let content = column![
        text("no downloads").size(14).color(p.text_tertiary),
        Space::new().height(4),
        text("add a url or paste from clipboard")
            .size(13)
            .color(p.text_ghost),
        Space::new().height(16),
        button(
            row![iced_fonts::bootstrap::plus().size(14), text("new").size(12)]
                .spacing(4)
                .align_y(iced::Alignment::Center),
        )
        .style(style::btn_primary(p))
        .on_press(Message::AddDownloadPressed)
        .padding([6, 12]),
    ]
    .align_x(iced::Alignment::Center)
    .max_width(280);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_widths_preserve_resized_fixed_columns() {
        let widths = DownloadColumnWidths {
            size: 180.0,
            ..DownloadColumnWidths::default()
        };

        let effective = effective_column_widths(widths, Some(1_200.0));

        assert_eq!(effective.size, 180.0);
    }

    #[test]
    fn effective_widths_shrink_name_to_keep_actions_visible() {
        let widths = DownloadColumnWidths::default();
        let available_width = COLUMN_LAYOUT_CHROME_WIDTH
            + widths.size
            + widths.progress
            + widths.speed
            + widths.eta
            + 120.0;

        let effective = effective_column_widths(widths, Some(available_width));

        assert_eq!(effective.name, 120.0);
    }
}
