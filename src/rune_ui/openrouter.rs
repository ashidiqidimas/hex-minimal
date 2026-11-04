//! Fork change: the Output pane's OpenRouter card, the key and the switch
//! that rewrites every dictation through it. Upstream configures rewriting
//! only through `hex settings`; this makes it a key and a toggle. Copy stays
//! untranslated because only this fork shows it.

use rune::{El, col};
use rune_kit::{Switch, TextInput};

use super::Ui;
use super::controls::{METRICS, Paint, card, setting_row};
use crate::dictation::openrouter;

pub(super) fn card_section(ui: &Ui, p: Paint) -> El {
    col()
        .key("openrouter")
        .gap(METRICS.label_gap)
        .child(
            p.section_label("Rewrite with OpenRouter")
                .key("label")
                .mx(METRICS.row_px),
        )
        .child(card(p, "card", vec![key_row(ui, p), toggle_row(ui, p)]))
}

fn key_row(ui: &Ui, p: Paint) -> El {
    let note = if openrouter::key_from_environment() {
        "Using OPENROUTER_API_KEY from the environment".to_string()
    } else {
        ui.modes
            .openrouter_key_note
            .clone()
            .unwrap_or_else(|| "Press Return to save".into())
    };
    let field = TextInput::new(&ui.modes.openrouter_key)
        .secure()
        .placeholder("sk-or-…")
        .width(Some(220.0))
        .on_edit(|ui: &mut Ui| &mut ui.modes.openrouter_key)
        .on_submit(|ui: &mut Ui, _| {
            let key = ui.modes.openrouter_key.text.clone();
            ui.modes.openrouter_key_note = Some(match openrouter::save_key(&key) {
                Ok(()) if key.trim().is_empty() => "Key removed".into(),
                Ok(()) => "Key saved".into(),
                Err(error) => format!("Couldn't save the key: {error}"),
            });
            ui.models.reload();
        })
        .build(p.k)
        .key("key-field");
    setting_row(p, "openrouter-key", "API key", Some(&note), field)
}

fn toggle_row(ui: &Ui, p: Paint) -> El {
    let mode = &ui.settings.dictation_processing.default_mode;
    let enabled = mode.post_processing.enabled;
    // The draft, not the file: this runs every frame.
    let has_key =
        openrouter::key_from_environment() || !ui.modes.openrouter_key.text.trim().is_empty();
    let model = mode
        .post_processing
        .model
        .clone()
        .unwrap_or_else(|| openrouter::DEFAULT_MODEL.into());
    let note = if has_key || enabled {
        model
    } else {
        "Save a key first".into()
    };
    let toggle = Switch::new(enabled)
        .disabled(!enabled && !has_key)
        .on_toggle(|ui: &mut Ui| {
            ui.update(|settings| {
                let processing = &mut settings.dictation_processing.default_mode.post_processing;
                processing.enabled = !processing.enabled;
            });
        })
        .build(p.k)
        .key("toggle");
    setting_row(
        p,
        "openrouter-enabled",
        "Rewrite every dictation",
        Some(&note),
        toggle,
    )
}
