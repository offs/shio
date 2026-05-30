use crate::message::Message;
use iced::Task;
use shio_core::WindowMaterialPreference;

#[cfg(target_os = "windows")]
const ACRYLIC_TINT: (u8, u8, u8, u8) = (12, 12, 14, 220);

#[cfg(target_os = "windows")]
pub(crate) fn apply_vibrancy(
    id: iced::window::Id,
    material: WindowMaterialPreference,
) -> Task<Message> {
    use window_vibrancy::apply_acrylic;

    iced::window::run(id, move |w| {
        clear_materials(w);
        match material {
            WindowMaterialPreference::Acrylic => match apply_acrylic(w, Some(ACRYLIC_TINT)) {
                Ok(()) => tracing::info!("acrylic applied"),
                Err(e) => tracing::warn!("acrylic failed: {e}"),
            },
            WindowMaterialPreference::Solid => {
                tracing::debug!("solid material applied by clearing window backdrop effects");
            },
        }
    })
    .discard()
}

#[cfg(target_os = "windows")]
fn clear_materials<W>(w: &W)
where
    W: iced::window::raw_window_handle::HasWindowHandle + ?Sized,
{
    if let Err(e) = window_vibrancy::clear_acrylic(w) {
        tracing::debug!("clear acrylic skipped: {e}");
    }
    if let Err(e) = window_vibrancy::clear_mica(w) {
        tracing::debug!("clear mica skipped: {e}");
    }
    if let Err(e) = window_vibrancy::clear_blur(w) {
        tracing::debug!("clear blur skipped: {e}");
    }
    if let Err(e) = window_vibrancy::clear_tabbed(w) {
        tracing::debug!("clear tabbed skipped: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn apply_vibrancy(
    _id: iced::window::Id,
    _material: WindowMaterialPreference,
) -> Task<Message> {
    Task::none()
}
