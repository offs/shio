use crate::message::Message;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell, renderer};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, mouse};
use iced_renderer::fallback;

/// Publishes on every event under the tiny-skia fallback; the view drops it once reported.
pub(crate) struct SoftwareProbe;

impl Widget<Message, Theme, Renderer> for SoftwareProbe {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(&mut self, _: &mut Tree, _: &Renderer, _: &layout::Limits) -> layout::Node {
        layout::Node::new(Size::ZERO)
    }

    fn draw(
        &self,
        _: &Tree,
        _: &mut Renderer,
        _: &Theme,
        _: &renderer::Style,
        _: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }

    fn update(
        &mut self,
        _: &mut Tree,
        _: &Event,
        _: Layout<'_>,
        _: mouse::Cursor,
        renderer: &Renderer,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _: &Rectangle,
    ) {
        if matches!(renderer, fallback::Renderer::Secondary(_)) {
            shell.publish(Message::SoftwareRendering);
        }
    }
}

impl From<SoftwareProbe> for Element<'_, Message> {
    fn from(probe: SoftwareProbe) -> Self {
        Self::new(probe)
    }
}
