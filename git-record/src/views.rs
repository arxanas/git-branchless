use cursive::{
    direction,
    event::{AnyCb, Callback, Event, EventResult, Key},
    view::{CannotFocus, IntoBoxedView, Selector, View, ViewNotFound},
    Cursive, Printer, Rect, Vec2, With,
};
use std::rc::Rc;
use tracing::debug;

/// Represents a child from a [`ListView`].
pub enum ListChild {
    /// A single row, with a label and a view.
    Row(String, Box<dyn View>),
    /// A delimiter between groups.
    Delimiter,
}

impl ListChild {
    fn label(&self) -> &str {
        match *self {
            ListChild::Row(ref label, _) => label,
            _ => "",
        }
    }

    fn view(&mut self) -> Option<&mut dyn View> {
        match *self {
            ListChild::Row(_, ref mut view) => Some(view.as_mut()),
            _ => None,
        }
    }
}

/// Displays a list of elements.
pub struct ListView2 {
    children: Vec<ListChild>,
    // Height for each child.
    // This should have the same size as the `children` list.
    children_heights: Vec<usize>,
    // Which child is focused? Should index into the `children` list.
    focus: usize,
    // This callback is called when the selection is changed.
    on_select: Option<Rc<dyn Fn(&mut Cursive, &String)>>,
}

// Implement `Default` around `ListView::new`
// new_default!(ListView);

impl ListView2 {
    /// Creates a new, empty `ListView`.
    pub fn new() -> Self {
        ListView2 {
            children: Vec::new(),
            children_heights: Vec::new(),
            focus: 0,
            on_select: None,
        }
    }

    /// Returns the number of children, including delimiters.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Returns `true` if this view contains no children.
    ///
    /// Returns `false` if at least a delimiter or a view is present.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Returns a reference to the children
    pub fn children(&self) -> &[ListChild] {
        &self.children[..]
    }

    /// Returns a reference to the child at the given position.
    pub fn get_row(&self, id: usize) -> &ListChild {
        &self.children[id]
    }

    /// Gives mutable access to the child at the given position.
    ///
    /// # Panics
    ///
    /// Panics if `id >= self.len()`.
    pub fn row_mut(&mut self, id: usize) -> &mut ListChild {
        &mut self.children[id]
    }

    /// Adds a view to the end of the list.
    pub fn add_child<V: IntoBoxedView + 'static>(&mut self, label: &str, view: V) {
        let view = view.into_boxed_view();

        // Why were we doing this here?
        // view.take_focus(direction::Direction::none());
        self.children.push(ListChild::Row(label.to_string(), view));
        self.children_heights.push(0);
    }

    /// Removes all children from this view.
    pub fn clear(&mut self) {
        self.children.clear();
        self.children_heights.clear();
    }

    /// Adds a view to the end of the list.
    ///
    /// Chainable variant.
    #[must_use]
    pub fn child<V: IntoBoxedView + 'static>(self, label: &str, view: V) -> Self {
        self.with(|s| s.add_child(label, view))
    }

    /// Adds a delimiter to the end of the list.
    pub fn add_delimiter(&mut self) {
        self.children.push(ListChild::Delimiter);
        self.children_heights.push(0);
    }

    /// Adds a delimiter to the end of the list.
    ///
    /// Chainable variant.
    #[must_use]
    pub fn delimiter(self) -> Self {
        self.with(Self::add_delimiter)
    }

    /// Removes a child from the view.
    ///
    /// # Panics
    ///
    /// If `index >= self.len()`.
    pub fn remove_child(&mut self, index: usize) -> ListChild {
        self.children_heights.remove(index);
        self.children.remove(index)
    }

    /// Sets a callback to be used when an item is selected.
    pub fn set_on_select<F>(&mut self, cb: F)
    where
        F: Fn(&mut Cursive, &String) + 'static,
    {
        self.on_select = Some(Rc::new(cb));
    }

    /// Sets a callback to be used when an item is selected.
    ///
    /// Chainable variant.
    #[must_use]
    pub fn on_select<F>(self, cb: F) -> Self
    where
        F: Fn(&mut Cursive, &String) + 'static,
    {
        self.with(|s| s.set_on_select(cb))
    }

    /// Returns the index of the currently focused item.
    ///
    /// Panics if the list is empty.
    pub fn focus(&self) -> usize {
        self.focus
    }

    fn iter_mut<'a>(
        &'a mut self,
        from_focus: bool,
        source: direction::Relative,
    ) -> Box<dyn Iterator<Item = (usize, &mut ListChild)> + 'a> {
        match source {
            direction::Relative::Front => {
                let start = if from_focus { self.focus } else { 0 };

                Box::new(self.children.iter_mut().enumerate().skip(start))
            }
            direction::Relative::Back => {
                let end = if from_focus {
                    self.focus + 1
                } else {
                    self.children.len()
                };
                Box::new(self.children[..end].iter_mut().enumerate().rev())
            }
        }
    }

    fn unfocus_child(&mut self) -> EventResult {
        self.children
            .get_mut(self.focus)
            .and_then(ListChild::view)
            .map(|v| v.on_event(Event::FocusLost))
            .unwrap_or(EventResult::Ignored)
    }

    // Move focus to the given index, regardless of whether that child accepts focus.
    fn set_focus_unchecked(&mut self, index: usize) -> EventResult {
        if index != self.focus {
            let res = self.unfocus_child();
            self.focus = index;
            res
        } else {
            EventResult::Consumed(None)
        }
    }

    fn move_focus(&mut self, n: usize, source: direction::Direction) -> EventResult {
        let (i, res) = if let Some((i, res)) = source
            .relative(direction::Orientation::Vertical)
            .and_then(|rel| {
                // The iterator starts at the focused element.
                // We don't want that one.
                self.iter_mut(true, rel)
                    .skip(1)
                    .filter_map(|p| try_focus(p, source))
                    .take(n)
                    .last()
            }) {
            (i, res)
        } else {
            return EventResult::Ignored;
        };
        self.set_focus_unchecked(i);

        res.and(EventResult::Consumed(self.on_select.clone().map(|cb| {
            let i = self.focus();
            let focused_string = String::from(self.children[i].label());
            Callback::from_fn(move |s| cb(s, &focused_string))
        })))
    }

    fn labels_width(&self) -> usize {
        self.children
            .iter()
            .map(ListChild::label)
            .map(|s| s.len())
            .max()
            .unwrap_or(0)
    }

    fn check_focus_grab(&mut self, event: &Event) -> Option<EventResult> {
        if let Event::Mouse {
            offset,
            position,
            event,
        } = *event
        {
            if !event.grabs_focus() {
                return None;
            }

            let mut position = match position.checked_sub(offset) {
                None => return None,
                Some(pos) => pos,
            };

            // eprintln!("Rel pos: {:?}", position);

            // Now that we have a relative position, checks for buttons?
            for (i, (child, &height)) in self
                .children
                .iter_mut()
                .zip(&self.children_heights)
                .enumerate()
            {
                if let Some(y) = position.y.checked_sub(height) {
                    // Not this child. Move on.
                    position.y = y;
                    continue;
                }

                // We found the correct target, try to focus it.
                if let ListChild::Row(_, ref mut view) = child {
                    match view.take_focus(direction::Direction::none()) {
                        Ok(res) => {
                            return Some(self.set_focus_unchecked(i).and(res));
                        }
                        Err(CannotFocus) => (),
                    }
                }
                // We found the target, but we can't focus it.
                break;
            }
        }
        None
    }
}

fn try_focus(
    (i, child): (usize, &mut ListChild),
    source: direction::Direction,
) -> Option<(usize, EventResult)> {
    match *child {
        ListChild::Delimiter => None,
        ListChild::Row(_, ref mut view) => match view.take_focus(source) {
            Ok(res) => Some((i, res)),
            Err(CannotFocus) => None,
        },
    }
}

impl View for ListView2 {
    fn draw(&self, printer: &Printer) {
        if self.children.is_empty() {
            return;
        }

        let offset = self.labels_width() + 1;
        let mut y = 0;

        debug!("Offset: {}", offset);
        for (i, (child, &height)) in self.children.iter().zip(&self.children_heights).enumerate() {
            match child {
                ListChild::Row(ref label, ref view) => {
                    printer.print((0, y), label);
                    view.draw(
                        &printer
                            .offset((offset, y))
                            .cropped((printer.size.x, height))
                            .focused(i == self.focus),
                    );
                }
                ListChild::Delimiter => y += 1, // TODO: draw delimiters?
            }
            y += height;
        }
    }

    fn required_size(&mut self, req: Vec2) -> Vec2 {
        // We'll show 2 columns: the labels, and the views.
        let label_width = self
            .children
            .iter()
            .map(ListChild::label)
            .map(|s| s.len())
            .max()
            .unwrap_or(0);

        let view_size =
            direction::Orientation::Vertical.stack(self.children.iter_mut().map(|c| match c {
                ListChild::Delimiter => Vec2::new(0, 1),
                ListChild::Row(_, ref mut view) => view.required_size(req),
            }));

        view_size + (1 + label_width, 0)
    }

    fn layout(&mut self, size: Vec2) {
        // We'll show 2 columns: the labels, and the views.
        let label_width = self
            .children
            .iter()
            .map(ListChild::label)
            .map(|s| s.len())
            .max()
            .unwrap_or(0);

        let spacing = 1;

        let available = size.x.saturating_sub(label_width + spacing);

        debug!("Available: {}", available);

        self.children_heights.resize(self.children.len(), 0);
        for (child, height) in self
            .children
            .iter_mut()
            .zip(&mut self.children_heights)
            .filter_map(|(v, h)| v.view().map(|v| (v, h)))
        {
            // TODO: Find the child height?
            *height = child.required_size(size).y;
            child.layout(Vec2::new(available, *height));
        }
    }

    fn on_event(&mut self, event: Event) -> EventResult {
        if self.children.is_empty() {
            return EventResult::Ignored;
        }

        let res = self
            .check_focus_grab(&event)
            .unwrap_or(EventResult::Ignored);

        // Send the event to the focused child.
        let labels_width = self.labels_width();
        if let ListChild::Row(_, ref mut view) = self.children[self.focus] {
            let y = self.children_heights[..self.focus].iter().sum();
            let offset = (labels_width + 1, y);
            let result = view.on_event(event.relativized(offset));
            if result.is_consumed() {
                return res.and(result);
            }
        }

        // If the child ignored this event, change the focus.
        res.and(match event {
            Event::Key(Key::Up) if self.focus > 0 => {
                self.move_focus(1, direction::Direction::down())
            }
            Event::Key(Key::Down) if self.focus + 1 < self.children.len() => {
                self.move_focus(1, direction::Direction::up())
            }
            Event::Key(Key::PageUp) => self.move_focus(10, direction::Direction::down()),
            Event::Key(Key::PageDown) => self.move_focus(10, direction::Direction::up()),
            Event::Key(Key::Home) | Event::Ctrl(Key::Home) => {
                self.move_focus(usize::max_value(), direction::Direction::back())
            }
            Event::Key(Key::End) | Event::Ctrl(Key::End) => {
                self.move_focus(usize::max_value(), direction::Direction::front())
            }
            Event::Key(Key::Tab) => self.move_focus(1, direction::Direction::front()),
            Event::Shift(Key::Tab) => self.move_focus(1, direction::Direction::back()),
            _ => EventResult::Ignored,
        })
    }

    fn take_focus(&mut self, source: direction::Direction) -> Result<EventResult, CannotFocus> {
        let rel = source.relative(direction::Orientation::Vertical);
        let (i, res) = if let Some((i, res)) = self
            .iter_mut(rel.is_none(), rel.unwrap_or(direction::Relative::Front))
            .find_map(|p| try_focus(p, source))
        {
            (i, res)
        } else {
            // No one wants to be in focus
            return Err(CannotFocus);
        };
        Ok(self.set_focus_unchecked(i).and(res))
    }

    fn call_on_any<'a>(&mut self, selector: &Selector<'_>, callback: AnyCb<'a>) {
        for view in self.children.iter_mut().filter_map(ListChild::view) {
            view.call_on_any(selector, callback);
        }
    }

    fn focus_view(&mut self, selector: &Selector<'_>) -> Result<EventResult, ViewNotFound> {
        // Try to focus each view. Skip over delimiters.
        if let Some((i, res)) = self
            .children
            .iter_mut()
            .enumerate()
            .filter_map(|(i, v)| v.view().map(|v| (i, v)))
            .find_map(|(i, v)| v.focus_view(selector).ok().map(|res| (i, res)))
        {
            Ok(self.set_focus_unchecked(i).and(res))
        } else {
            Err(ViewNotFound)
        }
    }

    fn important_area(&self, size: Vec2) -> Rect {
        if self.children.is_empty() {
            return Rect::from_size(Vec2::zero(), size);
        }

        let labels_width = self.labels_width();

        // This is the size of the focused view
        let area = match self.children[self.focus] {
            ListChild::Row(_, ref view) => {
                let available = Vec2::new(size.x.saturating_sub(labels_width + 1), 1);
                view.important_area(available) + (labels_width, 0)
            }
            ListChild::Delimiter => Rect::from_size((0, 0), (size.x, 1)),
        };

        // This is how far down the focused view is.
        // (The size of everything above.)
        let y_offset: usize = self.children_heights[..self.focus].iter().copied().sum();

        area + (0, y_offset)
    }
}
