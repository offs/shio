use super::state::Shio;
use iced::Theme;

impl Shio {
    pub(crate) fn theme(&self) -> Theme {
        self.theme.iced_theme.clone()
    }
}
