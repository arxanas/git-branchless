//! Utilities to render an interactive text-based user interface.
use cursive::theme::{Color, PaletteColor};
use cursive::{Cursive, CursiveRunnable, CursiveRunner};

/// Create an instance of a `CursiveRunner`, and clean it up afterward.
pub(crate) fn with_siv<T, F: FnOnce(CursiveRunner<CursiveRunnable>) -> anyhow::Result<T>>(
    f: F,
) -> anyhow::Result<T> {
    // I tried these back-ends:
    //
    // * `ncurses`/`pancurses`: Doesn't render ANSI escape codes. (NB: the fact
    //   that we print out strings with ANSI escape codes is tech debt; we would
    //   ideally pass around styled representations of all text, and decide how to
    //   rendering it later.) Rendered scroll view improperly. No mouse/scrolling
    //   support
    // * `termion`: Renders ANSI escape codes. Has mouse/scrolling support. But
    //   critical bug: https://github.com/gyscos/cursive/issues/563
    // * `crossterm`: Renders ANSI escape codes. Has mouse/scrolling support.
    //   However, has some flickering issues, particularly when scrolling. See
    //   issue at https://github.com/gyscos/cursive/issues/142. I tried the
    //   `cursive_buffered_backend` library, but this causes it to no longer
    //   respect the ANSI escape codes.
    // * `blt`: Seems to require that a certain library be present on the system
    //   for linking.
    let mut siv = cursive::crossterm();

    siv.update_theme(|theme| {
        theme.shadow = false;
        theme.palette.extend(vec![
            (PaletteColor::Background, Color::TerminalDefault),
            (PaletteColor::View, Color::TerminalDefault),
            (PaletteColor::Primary, Color::TerminalDefault),
        ]);
    });
    let old_max_level = log::max_level();
    log::set_max_level(log::LevelFilter::Off);
    let result = f(siv.into_runner());
    log::set_max_level(old_max_level);
    let result = result?;
    Ok(result)
}

/// Type-safe "singleton" view: a kind of view which is addressed by name, for
/// which exactly one copy exists in the Cursive application.
pub trait SingletonView<V> {
    /// Look up the instance of the singleton view in the application. Panics if
    /// it hasn't been added.
    fn find(siv: &mut Cursive) -> cursive::views::ViewRef<V>;
}

/// Create a set of views with unique names. See also `new_view!` and
/// `find_view!`.
///
/// ```
/// # use cursive::Cursive;
/// # use cursive::views::{EditView, TextView};
/// # use branchless::declare_views;
/// # use branchless::core::tui::SingletonView;
/// # fn main() {
/// declare_views! {
///     SomeDisplayView => TextView,
///     SomeDataEntryView => EditView,
/// }
/// let mut siv = Cursive::new();
/// siv.add_layer::<SomeDisplayView>(TextView::new("Hello, world!").into());
/// assert_eq!(SomeDisplayView::find(&mut siv).get_content().source(), "Hello, world!");
/// # }
/// ```
#[macro_export]
macro_rules! declare_views {
    { $( $k:ident => $v:ty ),* $(,)? } => {
        $(
            struct $k {
                view: cursive::views::NamedView<$v>,
            }

            impl $crate::core::tui::SingletonView<$v> for $k {
                fn find(siv: &mut Cursive) -> cursive::views::ViewRef<$v> {
                    siv.find_name::<$v>(stringify!($k)).unwrap()
                }
            }

            impl cursive::view::IntoBoxedView for $k {
                fn into_boxed_view(self) -> Box<dyn cursive::view::View> {
                    Box::new(self.view)
                }
            }

            impl From<$v> for $k {
                fn from(view: $v) -> Self {
                    use cursive::view::Nameable;
                    let view = view.with_name(stringify!($k));
                    $k { view }
                }
            }
        )*
    };
}
