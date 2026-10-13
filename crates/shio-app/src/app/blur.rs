use iced::Task;
use iced::window::{self, Window};

// Only ext-background-effect toggles live; macOS and KWin blur exist only if the window was
// created with it (`created`).
pub(super) fn apply(id: window::Id, on: bool, created: bool) -> Task<bool> {
    window::run(id, move |w| set(w, on, created))
}

#[cfg(target_os = "linux")]
fn set(w: &dyn Window, on: bool, created: bool) -> bool {
    super::wayland::set_blur(w, on, created)
}

#[cfg(target_os = "macos")]
const fn set(_: &dyn Window, on: bool, created: bool) -> bool {
    on && created
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const fn set(_: &dyn Window, _: bool, _: bool) -> bool {
    false
}
