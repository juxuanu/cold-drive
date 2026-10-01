//! Scrolling a widget into view: iced scrolls to offsets, not to widgets, so
//! an operation measures where the widget is first.

use iced::Task;
use iced::advanced::widget::operation::{Animation, Outcome, Scrollable};
use iced::advanced::widget::{self, Id, Operation};
use iced::widget::operation::scroll_to;
use iced::widget::scrollable::AbsoluteOffset;
use iced::{Rectangle, Size, Vector};

/// Scrolls `scrollable` the least that shows `target` whole, with `margin`
/// above and below it — the parts of what it stands for around it — or not
/// at all if it shows already.
pub fn reveal<T: Send + 'static>(scrollable: Id, target: Id, margin: f32) -> Task<T> {
    let measure = Measure {
        scrollable: scrollable.clone(),
        target,
        margin,
        view: None,
        content_top: None,
        found: None,
    };

    widget::operate(measure).then(move |y| {
        scroll_to(
            scrollable.clone(),
            AbsoluteOffset {
                x: None,
                y: Some(y),
            },
            Animation::Auto,
        )
    })
}

struct Measure {
    scrollable: Id,
    target: Id,
    margin: f32,
    /// The scrollable's bounds and how far it has scrolled.
    view: Option<(Rectangle, f32)>,
    /// Where its content starts, unscrolled.
    content_top: Option<f32>,
    found: Option<Rectangle>,
}

impl Operation<f32> for Measure {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content: Size,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.scrollable) {
            self.view = Some((bounds, translation.y));
        }
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, _viewport: &Rectangle) {
        // A scrollable reports its content as a container under its own id,
        // laid out where it would be unscrolled; so is everything in it.
        if id == Some(&self.scrollable) {
            self.content_top = Some(bounds.y);
        } else if id == Some(&self.target) {
            self.found = Some(bounds);
        }
    }

    fn finish(&self) -> Outcome<f32> {
        let (Some((view, offset)), Some(top), Some(target)) =
            (self.view, self.content_top, self.found)
        else {
            return Outcome::None;
        };

        match least_scroll(
            target.y - top,
            target.height,
            self.margin,
            offset,
            view.height,
        ) {
            Some(y) => Outcome::Some(y),
            None => Outcome::None,
        }
    }
}

/// The offset that brings `[start - margin, start + height + margin]` into a
/// view `page` tall now scrolled to `offset`, moving as little as it can;
/// `None` when it shows already.
fn least_scroll(start: f32, height: f32, margin: f32, offset: f32, page: f32) -> Option<f32> {
    let (top, bottom) = (start - margin, start + height + margin);

    if top < offset {
        Some(top.max(0.0))
    } else if bottom > offset + page {
        Some(bottom - page)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::least_scroll;

    #[test]
    fn scrolls_the_least_that_shows_the_target() {
        // In view: no scroll.
        assert_eq!(least_scroll(100.0, 20.0, 10.0, 0.0, 400.0), None);
        // Below: its bottom edge, margin included, at the view's bottom.
        assert_eq!(least_scroll(500.0, 20.0, 10.0, 0.0, 400.0), Some(130.0));
        // Above: its top edge, margin included, at the view's top.
        assert_eq!(least_scroll(100.0, 20.0, 10.0, 300.0, 400.0), Some(90.0));
        // Never above the content's start.
        assert_eq!(least_scroll(5.0, 20.0, 10.0, 300.0, 400.0), Some(0.0));
    }
}
