use crate::message::Message;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, renderer, text};
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{TextInput, text_input};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, mouse};

type InputState = text_input::State<<Renderer as text::Renderer>::Paragraph>;

/// Reports focus changes and, while suggestions are open, takes the keys that steer them.
pub(crate) fn search_field(
    input: TextInput<'_, Message>,
    focused: bool,
    suggesting: bool,
) -> Element<'_, Message> {
    Element::new(SearchField {
        input,
        focused,
        suggesting,
    })
}

struct SearchField<'a> {
    input: TextInput<'a, Message>,
    focused: bool,
    suggesting: bool,
}

impl Widget<Message, Theme, Renderer> for SearchField<'_> {
    fn tag(&self) -> tree::Tag {
        Widget::tag(&self.input)
    }

    fn state(&self) -> tree::State {
        Widget::state(&self.input)
    }

    fn diff(&self, tree: &mut Tree) {
        Widget::diff(&self.input, tree);
    }

    fn size(&self) -> Size<Length> {
        Widget::size(&self.input)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        Widget::layout(&mut self.input, tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        Widget::operate(&mut self.input, tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let focused = |tree: &Tree| tree.state.downcast_ref::<InputState>().is_focused();
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key:
                Key::Named(
                    key @ (Named::ArrowDown
                    | Named::ArrowUp
                    | Named::Enter
                    | Named::Tab
                    | Named::Escape),
                ),
            ..
        }) = event
            && self.suggesting
            && focused(tree)
        {
            match key {
                Named::ArrowDown => shell.publish(Message::HighlightSuggestion { down: true }),
                Named::ArrowUp => shell.publish(Message::HighlightSuggestion { down: false }),
                Named::Enter | Named::Tab => shell.publish(Message::AcceptSuggestion),
                _ => {},
            }
            shell.capture_event();
            return;
        }
        Widget::update(
            &mut self.input,
            tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let now = focused(tree);
        if now != self.focused {
            shell.publish(Message::SearchFocused(now));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        Widget::mouse_interaction(&self.input, tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        Widget::draw(
            &self.input,
            tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
}
