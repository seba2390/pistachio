//! Minimize, maximize and close at the right end of the top bar, where the system does not
//! draw them: on Windows, and on Linux when the app decorates its own window (Wayland).
//!
//! The window has a transparent, app-owned title bar (see [`super::WindowDrag`]), so nothing
//! else would show them. On Windows the buttons are hit-test areas: Windows itself presses
//! them (with its own hover delay, snap layouts on maximize and the close shortcut). On Linux
//! they act on a click.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::TOP_BAR_HEIGHT;

/// The width of one button (Windows' own caption buttons are 46 pixels wide).
const BUTTON_WIDTH: Pixels = px(46.);

/// Whether the app draws the buttons in this window.
fn shown(window: &Window) -> bool {
    if cfg!(target_os = "windows") {
        true
    } else if cfg!(target_os = "linux") {
        // With server-side decorations the window manager draws its own title bar.
        matches!(window.window_decorations(), Decorations::Client { .. })
    } else {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// The buttons this window has, left to right.
fn controls(window: &Window) -> Vec<Control> {
    if !shown(window) {
        return Vec::new();
    }
    let supported = window.window_controls();
    let mut controls = Vec::with_capacity(3);
    if supported.minimize {
        controls.push(Control::Minimize);
    }
    if supported.maximize {
        controls.push(if window.is_maximized() {
            Control::Restore
        } else {
            Control::Maximize
        });
    }
    controls.push(Control::Close);
    controls
}

/// Room the rightmost top bar keeps free for the buttons (none where the system draws them).
pub fn inset(window: &Window) -> Pixels {
    #[allow(clippy::cast_precision_loss)] // at most three buttons
    let count = controls(window).len() as f32;
    BUTTON_WIDTH * count
}

/// The buttons, to lay over the window's top right corner; `None` where the system draws them.
pub fn render(window: &Window, cx: &App) -> Option<AnyElement> {
    let controls = controls(window);
    if controls.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let foreground = theme.foreground;
    Some(
        h_flex()
            .id("window-controls")
            .absolute()
            .top_0()
            .right_0()
            .h(TOP_BAR_HEIGHT)
            .items_center()
            .children(controls.into_iter().map(|control| {
                let (id, icon, area) = match control {
                    Control::Minimize => (
                        "window-minimize",
                        IconName::WindowMinimize,
                        WindowControlArea::Min,
                    ),
                    Control::Maximize => (
                        "window-maximize",
                        IconName::WindowMaximize,
                        WindowControlArea::Max,
                    ),
                    Control::Restore => (
                        "window-restore",
                        IconName::WindowRestore,
                        WindowControlArea::Max,
                    ),
                    Control::Close => (
                        "window-close",
                        IconName::WindowClose,
                        WindowControlArea::Close,
                    ),
                };
                let (hover_bg, hover_fg) = if control == Control::Close {
                    (rgb(0x00c4_2b1c).into(), white())
                } else {
                    (theme.secondary_hover, theme.secondary_foreground)
                };
                let active_bg = if control == Control::Close {
                    rgb(0x00b2_2a1c).into()
                } else {
                    theme.secondary_active
                };
                div()
                    .id(id)
                    .flex()
                    .flex_none()
                    .w(BUTTON_WIDTH)
                    .h_full()
                    .items_center()
                    .justify_center()
                    .text_color(foreground)
                    .hover(move |style| style.bg(hover_bg).text_color(hover_fg))
                    .active(move |style| style.bg(active_bg).text_color(hover_fg))
                    .when(cfg!(target_os = "windows"), |this| {
                        this.window_control_area(area)
                    })
                    .when(cfg!(target_os = "linux"), |this| {
                        this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                            // Not the start of a window drag.
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            match control {
                                Control::Minimize => window.minimize_window(),
                                Control::Maximize | Control::Restore => window.zoom_window(),
                                Control::Close => window.remove_window(),
                            }
                        })
                    })
                    .child(Icon::new(icon).small())
            }))
            .into_any_element(),
    )
}
