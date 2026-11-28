//! Conversation names in Settings: the local models (download, use, remove) and a
//! notification when a download ends.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::catalog::{self, DEFAULT_MODEL, MODELS, Model};
use super::{Namer, models};
use crate::model_store::Downloadable as _;
use crate::settings::AppSettings;
use crate::theme::palette;
use crate::ui::AppIcon;
use crate::ui::download::{self, Details, Part, download_button};

/// Asks whether to download `model`, showing what that is.
pub fn ask_to_download(model: &'static Model, window: &mut Window, cx: &mut App) {
    download::ask_to_download(
        model.name,
        details(model),
        move |cx| start_download(model, cx),
        window,
        cx,
    );
}

fn details(model: &Model) -> Details {
    let (weights, tokenizer) = (&model.weights, &model.tokenizer);
    Details {
        intro: format!(
            "{} by {} names new conversations on this computer. {}.",
            model.name, model.maker, model.languages
        ),
        parts: vec![
            Part {
                link: None,
                icon: AppIcon::Brain.into(),
                name: model.name.into(),
                note: format!("The model · {}", weights.file.name),
                size: weights.file.size,
            },
            Part {
                link: None,
                icon: AppIcon::Library.into(),
                name: "Tokenizer".into(),
                note: format!(
                    "Splits text into the pieces the model reads · {}",
                    tokenizer.file.name
                ),
                size: tokenizer.file.size,
            },
        ],
        size: model.size(),
        // The files are used as they are.
        disk_size: model.size(),
        note: format!(
            "It comes from Hugging Face ({} and {}) and goes into Pistachio's own folder. \
             Nothing is sent anywhere for a name, and you can remove it at any time. License: {}.",
            weights.repo, tokenizer.repo, model.license
        ),
    }
}

/// Makes `model` the naming model (and local naming the namer) and starts downloading it.
pub fn start_download(model: &'static Model, cx: &mut App) {
    AppSettings::update(cx, |s| {
        model.id.clone_into(&mut s.naming_model);
        s.namer = Namer::Local;
    });
    models(cx).update(cx, |models, cx| models.start_download(model, cx));
}

/// Name (and "Recommended"), then languages · size, then the note and license.
fn model_summary(model: &'static Model, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = palette(cx).accent;
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(model.name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(model.maker),
                )
                .when(model.id == DEFAULT_MODEL, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .px_1p5()
                            .rounded_md()
                            .bg(accent.opacity(0.15))
                            .text_color(accent)
                            .child("Recommended"),
                    )
                }),
        )
        .child(div().text_xs().text_color(theme.foreground).child(format!(
            "{} · {}",
            model.languages,
            model.size_label()
        )))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{} License: {}.", model.note, model.license)),
        )
}

/// The local models, each with its state and what can be done with it.
pub fn settings_model_rows(cx: &App) -> AnyElement {
    let models = models(cx);
    let models = models.read(cx);
    let chosen = &cx.global::<AppSettings>().naming_model;
    let downloading = models.download().map(|download| download.model.id);
    let theme = cx.theme();
    let rows: Vec<AnyElement> = MODELS
        .iter()
        .map(|model| {
            let installed = models.is_installed(model.id);
            let in_use = installed && *chosen == model.id;
            let actions: AnyElement = if downloading == Some(model.id) {
                let percent = models.download().map_or(0., |d| d.fraction() * 100.);
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("Downloading… {percent:.0}%")),
                    )
                    .child(
                        Button::new(SharedString::from(format!("naming-cancel-{}", model.id)))
                            .label("Cancel")
                            .ghost()
                            .xsmall()
                            .on_click(|_, _, cx| {
                                super::models(cx).update(cx, |models, _| models.cancel_download());
                            }),
                    )
                    .into_any_element()
            } else if installed {
                h_flex()
                    .gap_2()
                    .child(if in_use {
                        div()
                            .text_xs()
                            .text_color(palette(cx).accent)
                            .child("In use")
                            .into_any_element()
                    } else {
                        Button::new(SharedString::from(format!("naming-use-{}", model.id)))
                            .label("Use")
                            .outline()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                AppSettings::update(cx, |s| {
                                    model.id.clone_into(&mut s.naming_model);
                                });
                            })
                            .into_any_element()
                    })
                    .child(
                        Button::new(SharedString::from(format!("naming-remove-{}", model.id)))
                            .label("Remove")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                super::models(cx)
                                    .update(cx, |models, cx| models.remove(model.id, cx));
                            }),
                    )
                    .into_any_element()
            } else {
                download_button(
                    format!("naming-get-{}", model.id),
                    model.name,
                    model.size(),
                    cx,
                )
                .disabled(downloading.is_some())
                .on_click(move |_, window, cx| ask_to_download(model, window, cx))
                .into_any_element()
            };
            h_flex()
                .w_full()
                .gap_3()
                .py_2()
                .child(model_summary(model, cx))
                .child(div().flex_none().child(actions))
                .into_any_element()
        })
        .collect();
    v_flex().w_full().children(rows).into_any_element()
}

/// A notification when a naming model download ends.
pub fn notify_download(event: &super::NamingModelsEvent, window: &mut Window, cx: &mut App) {
    use crate::model_store::StoreEvent;
    use gpui_kit::component::notification::Notification;
    let name = |id: &str| catalog::model(id).map_or("The model", |model| model.name);
    let notification = match event {
        StoreEvent::Installed(id) => Notification::success(format!(
            "{} is ready to name new conversations on this computer.",
            name(id)
        )),
        StoreEvent::Failed(id, error) => {
            Notification::error(format!("Downloading {} failed: {error}", name(id)))
        }
        StoreEvent::Cancelled => return,
    };
    window.push_notification(notification, cx);
}

#[cfg(test)]
mod tests {
    use super::{MODELS, details};

    #[test]
    fn the_details_are_all_that_is_downloaded() {
        for model in MODELS {
            details(model).assert_adds_up();
        }
    }
}
