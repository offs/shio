use super::super::state::{DragHover, DropSide, Shio};
use crate::message::Message;
use iced::Task;
use shio_core::DownloadId;

impl Shio {
    pub(super) fn drag_drop(&mut self, source_id: DownloadId) -> Task<Message> {
        if let Some(hover) = self.drag_hover.take()
            && let Some(target_id) = hover.target_id
        {
            self.reorder_download(source_id, target_id, hover.side);
        }
        Task::none()
    }

    pub(super) fn drag_zones_found(
        &mut self,
        source_id: DownloadId,
        zones: &[(iced::widget::Id, iced::Rectangle)],
    ) -> Task<Message> {
        let cursor_y = self.drag_hover.as_ref().map_or(0.0, |h| h.cursor_y);
        let (target_id, side) = match zones.first() {
            Some((zone_id, rect)) => {
                let id = self
                    .downloads
                    .iter()
                    .find(|d| crate::views::download_list::row_widget_id(d.id) == *zone_id)
                    .map(|d| d.id);
                let side = if cursor_y > rect.y + rect.height / 2.0 {
                    DropSide::Below
                } else {
                    DropSide::Above
                };
                (id, side)
            },
            None => (None, DropSide::Above),
        };
        self.drag_hover = Some(DragHover {
            source_id,
            target_id,
            side,
            cursor_y,
        });
        Task::none()
    }

    pub(super) fn drag_update(
        &mut self,
        source_id: DownloadId,
        point: iced::Point,
    ) -> Task<Message> {
        let prev = self.drag_hover.take();
        self.drag_hover = Some(DragHover {
            source_id,
            target_id: prev.as_ref().and_then(|h| h.target_id),
            side: prev.as_ref().map_or(DropSide::Above, |h| h.side),
            cursor_y: point.y,
        });
        iced_drop::zones_on_point(
            move |zones| Message::DragZonesFound(source_id, zones),
            point,
            None,
            None,
        )
    }
}
