//! Where a secondary click lands: iced's `mouse_area` says that one came,
//! not where, and inside a scrollable a widget only knows the cursor in its
//! content's coordinates. Wrapped around the whole window, this reports each
//! right press in the window's — after whatever it lands on has had it, so
//! the press on a row is heard first and its position second.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Meta, Operation, Tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Event, Length, Point, Rectangle, Size, Vector};

/// Reports every right press inside `content` through `on_right_press`, with
/// the point relative to the area and the area's size.
pub struct PointerArea<'a, Message, Theme, Renderer> {
    content: iced::Element<'a, Message, Theme, Renderer>,
    on_right_press: Box<dyn Fn(Point, Size) -> Message + 'a>,
}

pub fn pointer_area<'a, Message, Theme, Renderer>(
    content: impl Into<iced::Element<'a, Message, Theme, Renderer>>,
    on_right_press: impl Fn(Point, Size) -> Message + 'a,
) -> PointerArea<'a, Message, Theme, Renderer> {
    PointerArea {
        content: content.into(),
        on_right_press: Box::new(on_right_press),
    }
}

impl<Message, Theme, Renderer> Meta for PointerArea<'_, Message, Theme, Renderer> {}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PointerArea<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content.layout(&mut tree.children[0], renderer, limits);

        tree.size = tree.children[0].size;
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .operate(&mut tree.children[0], layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        // Whether or not the content took the press: a row takes it, and
        // its menu still has to open where it was.
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            let bounds = layout.bounds();
            if let Some(position) = cursor.position_in(bounds) {
                shell.publish((self.on_right_press)(position, bounds.size()));
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
