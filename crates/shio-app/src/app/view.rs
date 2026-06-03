use super::state::{NoticeAction, PersistentNotice, Shio, ToastKind};
use crate::message::Message;
use crate::style;
use crate::views;
use iced::widget::{Space, button, column, container, opaque, row, stack, text};
use iced::{Element, Length};

impl Shio {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        if let Some(problem) = &self.startup_problem {
            return startup_problem_view(problem, self.palette(), self.config.window.material);
        }

        let _span = tracing::trace_span!("view", downloads = self.downloads.len()).entered();
        let filtered = self.filtered_downloads();
        let sorted = self.sorted_downloads(filtered);
        let p = self.palette();

        let suggestions = if self.search_text.is_empty() {
            Vec::new()
        } else {
            crate::search::completions(&self.search_text)
        };
        let titlebar = views::titlebar::view(&self.search_text, p);
        let tab_bar = views::tab_bar::view(self.active_tab, &self.downloads, p);
        let download_list = views::download_list::view(
            &sorted,
            &self.selection,
            views::download_list::ViewOptions {
                sort_col: self.sort_column,
                sort_dir: self.sort_direction,
                has_search: !self.search_query.is_empty(),
                drag: self.drag_hover.as_ref(),
                widths: self.column_widths,
                available_width: download_list_width(self.window.size),
                scroll_long_names: self.config.scroll_long_names,
                carousel_offset: self.name_carousel_offset(),
            },
            p,
        );
        let status_bar = views::status_bar::view(&self.downloads, &self.config, p);

        let tab_separator = container(Space::new().height(1))
            .style(style::separator(p))
            .width(Length::Fill)
            .height(1);

        let content_pane = container(download_list)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([4, 12]);

        let layout = column![titlebar, tab_bar, tab_separator, content_pane, status_bar]
            .height(Length::Fill);

        let base = container(layout)
            .style(style::window_background(p, self.config.window.material))
            .width(Length::Fill)
            .height(Length::Fill);

        let dropdown_layer: Element<'_, Message> = if suggestions.is_empty() {
            Space::new().width(0).height(0).into()
        } else {
            let dropdown = views::search_dropdown::view(&suggestions, self.suggestion_index, p);
            container(column![
                Space::new().height(crate::views::titlebar::TITLEBAR_HEIGHT),
                container(opaque(dropdown)).center_x(Length::Fill),
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        let main_content: Element<'_, Message> = stack![base, dropdown_layer].into();

        let main_or_modal_content: Element<'_, Message> = if self.show_first_run() {
            views::welcome::view(
                &self.config,
                self.first_run_step,
                &self.theme_catalog,
                p,
                self.config.window.material,
                main_content,
            )
        } else if self.show_add_dialog() {
            views::add_dialog::view(
                &views::add_dialog::AddDialogViewModel {
                    urls: &self.add_urls,
                    url_entries: &self.add_url_entries,
                    url_preview: &self.add_url_preview,
                    url_count: self.add_url_entries.len(),
                    http_count: self.add_http_count,
                    magnet_count: self.add_magnet_count,
                    addable_url_count: self.addable_add_url_count(),
                    single_url_name: self.add_single_url_name.as_deref(),
                    has_archive_url: self.add_has_archive_url,
                    http_previews: &self.add_http_previews,
                    magnet_previews: &self.add_magnet_previews,
                    torrent_files: &self.add_torrent_files,
                    torrent_search: &self.add_torrent_search,
                    selected_source: self.add_selected_source.as_ref(),
                    filename: &self.add_filename,
                    save_path: &self.add_save_path,
                    create_subfolder: self.add_create_subfolder,
                    subfolder_name: &self.add_subfolder_name,
                },
                p,
                self.config.window.material,
                main_content,
            )
        } else if self.show_settings() {
            views::settings::view(
                &self.config,
                self.settings_category,
                &self.settings_search,
                views::settings::TorrentSettingsInputs {
                    port: &self.torrent_port_input,
                    port_error: self.torrent_port_error.as_deref(),
                    ratio: &self.torrent_ratio_input,
                    ratio_error: self.torrent_ratio_error.as_deref(),
                    seed_days: &self.torrent_seed_days_input,
                    seed_days_error: self.torrent_seed_days_error.as_deref(),
                },
                &self.theme_catalog,
                p,
                self.config.window.material,
                main_content,
            )
        } else if let Some(id) = self.password_prompt() {
            if let Some(dl) = self.downloads.iter().find(|d| d.id == id) {
                views::password_dialog::view(
                    id,
                    &dl.filename,
                    &self.password_input,
                    p,
                    self.config.window.material,
                    main_content,
                )
            } else {
                main_content
            }
        } else if let Some(targets) = self.cancel_confirm_targets() {
            let (label, multiple) = match targets {
                [] => (String::new(), false),
                [id] => (
                    self.downloads
                        .iter()
                        .find(|d| d.id == *id)
                        .map(|d| d.filename.clone())
                        .unwrap_or_default(),
                    false,
                ),
                many => (format!("{} downloads", many.len()), true),
            };
            views::confirm_cancel::view(
                label,
                multiple,
                p,
                self.config.window.material,
                main_content,
            )
        } else if let Some(targets) = self.delete_confirm_targets() {
            let (label, multiple) = match targets {
                [] => (String::new(), false),
                [id] => (
                    self.downloads
                        .iter()
                        .find(|d| d.id == *id)
                        .map(|d| d.filename.clone())
                        .unwrap_or_default(),
                    false,
                ),
                many => (format!("{} downloads", many.len()), true),
            };
            views::confirm_delete::view(
                label,
                multiple,
                p,
                self.config.window.material,
                main_content,
            )
        } else if let Some(id) = self.edit_target() {
            if let Some(dl) = self.downloads.iter().find(|d| d.id == id) {
                let sanitized = shio_core::sanitize_filename(&self.edit_filename);
                let sanitized_hint = if sanitized == self.edit_filename {
                    None
                } else {
                    Some(sanitized)
                };
                views::edit_dialog::view(
                    id,
                    dl.status,
                    &self.edit_filename,
                    &self.edit_save_path,
                    self.edit_conflict,
                    sanitized_hint,
                    p,
                    self.config.window.material,
                    main_content,
                )
            } else {
                main_content
            }
        } else {
            main_content
        };

        let main_with_notices: Element<'_, Message> = if self.persistent_notices.is_empty() {
            main_or_modal_content
        } else {
            stack![
                main_or_modal_content,
                notice_overlay(&self.persistent_notices, p),
            ]
            .into()
        };

        if self.toasts.is_empty() {
            main_with_notices
        } else {
            let now = self.now;
            let toast_list = column(self.toasts.iter().map(|toast| {
                let accent = match toast.kind {
                    ToastKind::Success => p.success,
                    ToastKind::Error => p.error,
                    ToastKind::Info => p.accent,
                };
                let alpha = toast.shown.interpolate(0.0_f32, 1.0_f32, now);
                let slide = toast.shown.interpolate(12.0_f32, 0.0_f32, now);
                iced::widget::opaque(
                    container(
                        container(
                            row![text(&toast.message).size(13)]
                                .spacing(8)
                                .align_y(iced::Alignment::Center),
                        )
                        .padding([10, 16])
                        .max_width(360)
                        .style(style::toast(p, accent.scale_alpha(alpha))),
                    )
                    .padding(iced::Padding::new(0.0).bottom(slide)),
                )
            }))
            .spacing(8)
            .align_x(iced::Alignment::End);

            let toast_overlay = container(toast_list)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Bottom)
                .padding(20);

            stack![main_with_notices, toast_overlay].into()
        }
    }
}

fn download_list_width(window_size: Option<iced::Size>) -> Option<f32> {
    let size = window_size?;
    Some((size.width - CONTENT_PANE_HORIZONTAL_PADDING).max(0.0))
}

const CONTENT_PANE_HORIZONTAL_PADDING: f32 = 24.0;

fn notice_overlay<'a>(
    notices: &'a [PersistentNotice],
    p: &'a crate::style::Palette,
) -> Element<'a, Message> {
    let notices = column(notices.iter().map(|notice| {
        let dismiss = button(text("dismiss").size(12))
            .style(style::btn_ghost(p))
            .on_press(Message::DismissNotice(notice.id))
            .padding([6, 10]);

        let action: Element<'a, Message> = match notice.action {
            Some(NoticeAction::OpenLogs) => button(text("open logs").size(12))
                .style(style::btn_primary(p))
                .on_press(Message::OpenLogsFolder)
                .padding([6, 10])
                .into(),
            None => Space::new().width(0).height(0).into(),
        };

        opaque(
            container(
                row![
                    column![
                        text(&notice.title).size(13).color(p.text_primary),
                        text(&notice.message).size(12).color(p.text_secondary),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    action,
                    dismiss,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding([10, 12])
            .max_width(720)
            .style(style::toast(p, p.warning)),
        )
    }))
    .spacing(8);

    container(notices)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Top)
        .padding([44, 20])
        .into()
}

fn startup_problem_view<'a>(
    problem: &'a super::state::StartupProblem,
    p: &'a crate::style::Palette,
    material: shio_core::WindowMaterialPreference,
) -> Element<'a, Message> {
    let open_logs = button(text("open logs").size(13))
        .style(style::btn_primary(p))
        .on_press(Message::OpenLogsFolder)
        .padding([8, 14]);

    let content = column![
        text(&problem.title).size(18).color(p.text_primary),
        Space::new().height(8),
        text(&problem.message).size(13).color(p.text_secondary),
        Space::new().height(16),
        text(&problem.path_label).size(11).color(p.text_tertiary),
        text(problem.db_path.to_string_lossy().to_string())
            .size(12)
            .color(p.text_secondary),
        Space::new().height(10),
        text("logs").size(11).color(p.text_tertiary),
        text(problem.log_path.to_string_lossy().to_string())
            .size(12)
            .color(p.text_secondary),
        Space::new().height(18),
        row![open_logs],
    ]
    .spacing(2)
    .max_width(620);

    container(content)
        .style(style::window_background(p, material))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(40)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}
