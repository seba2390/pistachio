//! Small, app-specific UI building blocks shared by the views.

pub mod backdrop;
mod button;
mod copy_button;
pub mod dialog;
pub mod download;
mod drag;
mod fading_text;
mod hover_switch;
mod icons;
pub mod knob;
pub(crate) mod list;
mod menu;
mod message_scroller;
mod pi_field;
pub mod spinner;
mod text;
pub mod window_controls;

pub use button::{QuietButton, buttons};
pub use copy_button::CopyButton;
pub use drag::WindowDrag;
pub use fading_text::FadingText;
pub use hover_switch::hover_switch;
pub use icons::AppIcon;
pub use menu::described_item;
pub use message_scroller::{MessageScroller, MessageScrollerState};
pub use pi_field::PiField;
pub use text::{relative_time, short_path};

/// Height of the top bar of every column (sidebar, chat, right panel), so their
/// separators line up like one title bar.
pub const TOP_BAR_HEIGHT: gpui_kit::Pixels = gpui_kit::px(44.);

/// Horizontal space reserved for macOS traffic lights in the leftmost top bar.
pub const TRAFFIC_LIGHT_INSET: gpui_kit::Pixels = if cfg!(target_os = "macos") {
    gpui_kit::px(76.)
} else {
    gpui_kit::px(12.)
};

/// Icon size of the top-bar buttons (sidebar, terminal, diff, files, panel toggles).
pub fn top_bar_icon(icon: impl Into<gpui_kit::component::Icon>) -> gpui_kit::component::Icon {
    use gpui_kit::Styled as _;
    icon.into().size_5()
}

/// A small status light.
pub fn light(color: gpui_kit::Hsla) -> gpui_kit::Div {
    use gpui_kit::Styled as _;
    gpui_kit::div()
        .size(gpui_kit::px(7.))
        .flex_none()
        .rounded_full()
        .bg(color)
}

/// `text` that opens `url` in the browser when clicked.
pub fn web_link(
    id: impl Into<gpui_kit::ElementId>,
    text: impl Into<gpui_kit::SharedString>,
    url: String,
    cx: &gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    };
    // Underlined, with the usual arrow out of a box: it opens in the browser.
    let theme = cx.theme();
    let (line, strong) = (theme.muted_foreground, theme.foreground);
    gpui_kit::div()
        .id(id.into())
        .min_w_0()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .child(
            gpui_kit::div()
                .min_w_0()
                .truncate()
                .underline()
                .text_decoration_color(line)
                .child(text.into()),
        )
        .child(
            gpui_kit::component::Icon::from(AppIcon::ExternalLink)
                .size_3()
                .flex_none()
                .text_color(line),
        )
        .hover(move |style| style.text_decoration_color(strong))
        .on_click(move |_, _, cx| cx.open_url(&url))
}
