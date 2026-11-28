//! The side chat: a second conversation in its own column, to the right of the visible one,
//! that starts with everything that one holds at that moment, also while it works (the tab on
//! the chat's right edge, `/side`, ⌘⇧N). Its session is written by
//! [`PiSession::side_chat`](crate::chat::session::PiSession::side_chat); after that it is an
//! ordinary session of its own, listed under the original in the sidebar.

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MainWindow, focus_chat, full, keeps_working, session_key, set_draft, take_draft};
use crate::actions::OpenSideChat;
use crate::chat::session::SideChatStart;
use crate::chat::{ChatEvent, ChatView, ComposerEvent};
use crate::ui::{AppIcon, QuietButton as _, TOP_BAR_HEIGHT};

/// The chat in the side column.
pub(super) struct SideChat {
    pub(super) chat: Entity<ChatView>,
    _subscription: Subscription,
}

impl MainWindow {
    /// Opens a side chat of the visible conversation, or of the side chat when that has the
    /// keyboard (its `/side`). One that is open already gives way to the new one.
    pub(super) fn open_side_chat(
        &mut self,
        _: &OpenSideChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = match &self.side {
            Some(side) if side.chat.focus_handle(cx).contains_focused(window, cx) => {
                side.chat.clone()
            }
            _ if self.welcome || self.page_open() => return,
            _ => self.chat.clone(),
        };
        self.start_side_chat(&from, window, cx);
    }

    /// Writes the side chat of `from` and shows it once it is written.
    fn start_side_chat(
        &mut self,
        from: &Entity<ChatView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.side_opening.is_some() {
            return;
        }
        let session = from.read(cx).session().clone();
        let task = session.update(cx, |session, cx| session.side_chat(cx));
        self.side_opening = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.side_opening = None;
                match result {
                    Ok(start) => this.show_side_chat(start, window, cx),
                    Err(message) => {
                        window.push_notification(Notification::warning(message), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn show_side_chat(
        &mut self,
        start: SideChatStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SideChatStart {
            cwd,
            file,
            host,
            model,
            thinking_level,
            inherited_prompts,
        } = start;
        let chat = cx.new(|cx| {
            let mut chat = ChatView::new(cwd, Some(file), host.clone(), window, cx);
            chat.set_inherited_prompts(inherited_prompts, cx);
            chat
        });
        let session = chat.read(cx).session().clone();
        session.update(cx, |session, cx| session.inherit(model, thinking_level, cx));
        self.set_side(Some(chat), window, cx);
        self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
        if let Some(host) = host {
            crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.refresh_sessions(&host, cx));
        }
    }

    /// Puts `chat` in the side column, or closes it (`None`). The chat there before keeps
    /// working in the background while it has work, like any chat switched away from, and what
    /// was typed in it is kept for its session.
    pub(super) fn set_side(
        &mut self,
        chat: Option<Entity<ChatView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(old) = self.side.take().map(|side| side.chat) {
            old.update(cx, |old, cx| old.set_visible(false, window, cx));
            if keeps_working(&old, cx) {
                let subscription = cx.subscribe_in(&old, window, Self::on_background_event);
                self.background.push((old, subscription));
            } else {
                self.keep_draft(&old, window, cx);
            }
        }
        match chat {
            Some(chat) => {
                self.background
                    .retain(|(background, _)| background.entity_id() != chat.entity_id());
                chat.update(cx, |chat, cx| chat.set_visible(true, window, cx));
                self.restore_draft(&chat, window, cx);
                if let Some(key) = session_key(&chat, cx) {
                    self.finished.remove(&key);
                }
                focus_chat(&chat, window, cx);
                let subscription = cx.subscribe_in(&chat, window, Self::on_side_event);
                self.side = Some(SideChat {
                    chat,
                    _subscription: subscription,
                });
            }
            None => focus_chat(&self.chat.clone(), window, cx),
        }
        self.sync_side_active(cx);
        self.update_activity(cx);
        cx.notify();
    }

    /// Shows the side chat in the main column instead (the side column closes).
    fn promote_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(side) = self.side.take() else {
            return;
        };
        self.sync_side_active(cx);
        self.close_pages(cx);
        self.show_chat(side.chat, window, cx);
    }

    fn on_side_event(
        &mut self,
        chat: &Entity<ChatView>,
        event: &ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChatEvent::SessionChanged => {
                self.sync_side_active(cx);
                self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
                self.update_activity(cx);
                let host = chat.read(cx).session().read(cx).host().cloned();
                if let Some(host) = host {
                    crate::hosts::hosts(cx)
                        .update(cx, |hosts, cx| hosts.refresh_sessions(&host, cx));
                }
            }
            ChatEvent::Reconnect => self.restart_side_chat(window, cx),
            ChatEvent::BusyChanged | ChatEvent::NeedsAttention | ChatEvent::Stopped => {
                self.update_activity(cx);
            }
            ChatEvent::Composer(ComposerEvent::Models) => {
                // Where the side chat's pi takes its models from, as for the main chat.
                let host = chat
                    .read(cx)
                    .session()
                    .read(cx)
                    .host()
                    .filter(|host| !crate::hosts::hosts(cx).read(cx).uses_local_models(host))
                    .cloned();
                self.open_models_dialog(super::ModelsOpen::Start, host, window, cx);
            }
            // A resumed session's folder and machine are fixed.
            ChatEvent::Composer(_) => {}
        }
    }

    /// Starts the side chat's pi again on its session (it stopped, or its connection did).
    fn restart_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(side) = &self.side else {
            return;
        };
        let old = side.chat.clone();
        let (cwd, file, host) = {
            let session = old.read(cx).session().read(cx);
            (
                session.cwd().to_path_buf(),
                session.session_file(),
                session.host().cloned(),
            )
        };
        let draft = take_draft(&old, window, cx);
        let inherited = old.read(cx).inherited();
        let chat = cx.new(|cx| {
            let mut chat = ChatView::new(cwd, file, host, window, cx);
            if let Some(inherited) = inherited {
                chat.set_inherited_prompts(inherited.prompts, cx);
            }
            chat
        });
        set_draft(&chat, draft, window, cx);
        // The old one's pi has stopped: nothing to keep running.
        self.side = None;
        self.set_side(Some(chat), window, cx);
    }

    /// The side chat's session, as the sidebar identifies it.
    pub(super) fn side_key(
        &self,
        cx: &App,
    ) -> Option<(Option<remote::HostId>, std::path::PathBuf)> {
        self.side
            .as_ref()
            .and_then(|side| session_key(&side.chat, cx))
    }

    pub(super) fn side_chat(&self) -> Option<&Entity<ChatView>> {
        self.side.as_ref().map(|side| &side.chat)
    }

    fn sync_side_active(&self, cx: &mut Context<Self>) {
        let key = self.side_key(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_side_active(key, cx));
    }

    /// The side column: its own top bar (what it is, its name, "show in the main column" and
    /// close) above the chat.
    /// `controls` is the room to keep free at the right end of its top bar for the window's
    /// buttons, when it is the rightmost column.
    pub(super) fn render_side_column(
        &self,
        controls: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chat = self.side_chat()?.clone();
        let session = chat.read(cx).session().read(cx);
        let title = session
            .display_title()
            .unwrap_or_else(|| "Side chat".to_owned());
        let theme = cx.theme();
        let header = self.drag.apply(
            h_flex()
                .id("side-top-bar")
                .h(TOP_BAR_HEIGHT)
                .flex_none()
                .w_full()
                .pl_4()
                .pr(px(8.) + controls)
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.secondary)
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Side chat"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .text_color(theme.foreground)
                        .child(title),
                )
                .child(
                    Button::new("side-promote")
                        .quiet(cx)
                        .compact()
                        .child(crate::ui::top_bar_icon(IconName::ArrowLeft))
                        .accessibility_label("Show in the main column")
                        .tooltip("Show in the main column")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.promote_side_chat(window, cx);
                        })),
                )
                .child(
                    Button::new("side-close")
                        .quiet(cx)
                        .compact()
                        .child(crate::ui::top_bar_icon(IconName::Close))
                        .accessibility_label("Close the side chat (it stays in the sidebar)")
                        .tooltip("Close the side chat (it stays in the sidebar)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_side(None, window, cx);
                        })),
                ),
        );
        Some(
            v_flex()
                .size_full()
                .min_w_0()
                .overflow_hidden()
                .border_l_1()
                .border_color(theme.border)
                .child(header)
                .child(div().flex_1().min_h_0().child(chat.cached(full())))
                .into_any_element(),
        )
    }

    /// The tab on the chat column's right edge, centred, that opens a side chat. Only for a
    /// conversation with something in it.
    pub(super) fn render_side_tab(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.welcome || self.page_open() {
            return None;
        }
        let session = self.chat.read(cx).session().read(cx);
        if session.is_idle() || (session.is_empty() && !session.is_busy()) {
            return None;
        }
        let opening = self.side_opening.is_some();
        let theme = cx.theme();
        let accent = crate::theme::palette(cx).accent;
        let (bg, hover, border, muted, fg) = (
            theme.background,
            theme.secondary_hover,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        );
        let tip = if opening {
            "Opening the side chat…"
        } else {
            "Start a new chat from this point in the conversation, with everything so far \
             (⌘⇧N or /side)"
        };
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .flex()
                .items_center()
                .child(
                    h_flex()
                        .id("side-chat-tab")
                        .role(Role::Button)
                        .aria_label(tip)
                        .h(px(30.))
                        .pl_2()
                        .pr_1p5()
                        .rounded_l_lg()
                        .border_1()
                        .border_r_0()
                        .border_color(border)
                        .bg(bg)
                        .shadow_sm()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(muted)
                        .when(!opening, |this| {
                            this.cursor_pointer()
                                .hover(move |style| style.bg(hover).text_color(fg))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let chat = this.chat.clone();
                                    this.start_side_chat(&chat, window, cx);
                                }))
                        })
                        .tooltip(move |window, cx| {
                            component::tooltip::Tooltip::new(tip).build(window, cx)
                        })
                        // An L-shaped arrow: a new chat branching off from here.
                        .child(
                            component::Icon::from(AppIcon::CornerDownRight)
                                .small()
                                .text_color(if opening { muted } else { accent }),
                        ),
                )
                .into_any_element(),
        )
    }
}
