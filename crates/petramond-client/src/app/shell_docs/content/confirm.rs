//! The browser's confirm page: an in-panel page, never a modal window, for
//! everything that destroys or cannot be taken back without a restart —
//! deleting a pack, replacing a hand-installed copy, withdrawing an install
//! another waiting install needs, and ending the game while downloads run.
//!
//! Enter is the right-hand button; Escape is Cancel.

use petramond::content::ListingRow;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

use super::super::{ScreenCtx, ShellCommand};
use super::rows::{cap, name_list, Entry, NAME_CAP};
use crate::app::content::{ContentSession, PendingKind};
use crate::app::ExitKind;

/// Which buttons the page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Variant {
    /// Cancel left, a danger action right.
    Destroy,
    /// A danger "now" far left; Cancel and a benign "when done" right.
    Exit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Confirmed {
    Delete { dir: String },
    Replace { row: ListingRow },
    Undo { dir: String },
    Exit { kind: ExitKind },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct ConfirmPage {
    pub(in crate::app) question: String,
    pub(in crate::app) body: String,
    pub(in crate::app) variant: Variant,
    /// The right-hand (danger or benign) button's text.
    pub(in crate::app) action_text: &'static str,
    /// The exit variant's far-left danger text.
    pub(in crate::app) now_text: &'static str,
    pub(in crate::app) confirmed: Confirmed,
}

pub(super) fn populate(page: Option<&ConfirmPage>, state: &mut UiState) {
    let variant = page.map(|p| p.variant);
    state.set("list_page", UiValue::Bool(page.is_none()));
    state.set("confirm_page", UiValue::Bool(page.is_some()));
    state.set(
        "confirming_destroy",
        UiValue::Bool(variant == Some(Variant::Destroy)),
    );
    // The benign-only variant waits for the listing to name dependencies
    // ("Get all"); nothing opens it yet.
    state.set("confirming_go", UiValue::Bool(false));
    state.set(
        "confirming_exit",
        UiValue::Bool(variant == Some(Variant::Exit)),
    );
    state.set("cancel_left", UiValue::Bool(variant != Some(Variant::Exit)));
    state.set("shows_go", UiValue::Bool(variant == Some(Variant::Exit)));
    let text = |f: fn(&ConfirmPage) -> String| page.map(f).unwrap_or_default();
    state.set(
        "confirm_question",
        UiValue::Str(text(|p| p.question.clone())),
    );
    state.set("confirm_body", UiValue::Str(text(|p| p.body.clone())));
    let destroy = matches!(variant, Some(Variant::Destroy));
    state.set(
        "confirm_action_text",
        UiValue::Str(if destroy {
            text(|p| p.action_text.into())
        } else {
            String::new()
        }),
    );
    state.set(
        "confirm_go_text",
        UiValue::Str(if destroy {
            String::new()
        } else {
            text(|p| p.action_text.into())
        }),
    );
    state.set(
        "confirm_now_text",
        UiValue::Str(text(|p| p.now_text.into())),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "confirm_cancel" => close(ctx.content),
            "confirm_danger" | "confirm_go" => accept(ctx, false),
            "confirm_now" => accept(ctx, true),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => accept(ctx, false),
        _ => {}
    }
}

fn close(content: &mut ContentSession) {
    if let Some(view) = content.view.as_mut() {
        view.confirm = None;
    }
}

fn open(content: &mut ContentSession, page: ConfirmPage) {
    if let Some(view) = content.view.as_mut() {
        view.confirm = Some(page);
    }
}

/// Do what the page asked. `now` is the exit variant's far-left button.
fn accept(ctx: &mut ScreenCtx, now: bool) {
    let content = &mut *ctx.content;
    let Some(page) = content.view.as_mut().and_then(|v| v.confirm.take()) else {
        return;
    };
    match page.confirmed {
        Confirmed::Delete { dir } => {
            if let Err(why) = content.remove(&dir) {
                content.failed.insert(dir, why);
            }
        }
        Confirmed::Replace { row } => {
            if let Err(why) = content.get(&row) {
                content.failed.insert(row.mod_id.clone(), why);
            }
        }
        Confirmed::Undo { dir } => content.undo(&dir),
        Confirmed::Exit { kind } if now || content.jobs.is_empty() => {
            ctx.request(ShellCommand::ExitNow(kind))
        }
        Confirmed::Exit { kind } => content.exit_when_idle = Some(kind),
    }
}

/// Every name that needs `id`: installed packs and pending installs.
fn dependents(content: &ContentSession, id: &str, pending_only: bool) -> Vec<String> {
    let view = content.view.as_ref();
    let installed = view
        .filter(|_| !pending_only)
        .into_iter()
        .flat_map(|v| v.locals.iter())
        .filter(|l| l.dependencies.iter().any(|d| d == id))
        .map(|l| l.name.clone());
    let pending = content.pending.iter().filter_map(|p| match &p.change {
        PendingKind::Install {
            name, dependencies, ..
        } if dependencies.iter().any(|d| d == id) => Some(name.clone()),
        _ => None,
    });
    let mut names: Vec<String> = installed.chain(pending).collect();
    names.sort();
    names.dedup();
    names
}

fn needs_sentence(names: &[String], consequence: &str) -> String {
    let verb = if names.len() == 1 { "needs" } else { "need" };
    format!("{} {verb} it and {consequence}.", name_list(names))
}

/// The saves whose recorded packs include `id` and that do not switch it
/// off: a few small reads, done when the confirm opens.
fn worlds_using(id: &str) -> Vec<String> {
    if cfg!(test) {
        return Vec::new();
    }
    let Ok(worlds) = petramond::save::list_worlds() else {
        return Vec::new();
    };
    worlds
        .into_iter()
        .filter(|w| {
            petramond::modding::modset::recorded_ids(&petramond::save::world_dir(&w.dir_name))
                .is_some_and(|ids| ids.contains(id))
                && !petramond::save::read_world_settings(&w.dir_name)
                    .disabled_mods
                    .contains(id)
        })
        .map(|w| w.name)
        .collect()
}

pub(in crate::app) fn delete_body(
    touches_world: bool,
    worlds: &[String],
    dependents: &[String],
) -> String {
    let mut body = if touches_world {
        "It is removed when Petramond restarts. Its blocks and items disappear from worlds that use it."
            .to_owned()
    } else {
        "It is removed when Petramond restarts. Your worlds are not affected.".to_owned()
    };
    if touches_world && !worlds.is_empty() {
        let noun = if worlds.len() == 1 { "world" } else { "worlds" };
        body.push_str(&format!(
            "\nUsed by {} {noun}: {}",
            worlds.len(),
            name_list(worlds)
        ));
    }
    if !dependents.is_empty() {
        body.push('\n');
        body.push_str(&needs_sentence(dependents, "will stop loading"));
    }
    body
}

pub(super) fn open_delete(content: &mut ContentSession, entry: &Entry) {
    let id = entry.key.clone();
    let worlds = if entry.touches_world {
        worlds_using(&id)
    } else {
        Vec::new()
    };
    let body = delete_body(
        entry.touches_world,
        &worlds,
        &dependents(content, &id, false),
    );
    open(
        content,
        ConfirmPage {
            question: format!("Delete {}?", cap(&entry.name, NAME_CAP)),
            body,
            variant: Variant::Destroy,
            action_text: "Delete",
            now_text: "",
            confirmed: Confirmed::Delete {
                dir: entry.dir.clone(),
            },
        },
    );
}

pub(super) fn open_replace(content: &mut ContentSession, entry: &Entry, row: ListingRow) {
    open(
        content,
        ConfirmPage {
            question: format!(
                "Replace your copy of {} with the petramond.com version?",
                cap(&entry.name, NAME_CAP)
            ),
            body: "Your copy is deleted when Petramond restarts.".to_owned(),
            variant: Variant::Destroy,
            action_text: "Replace",
            now_text: "",
            confirmed: Confirmed::Replace { row },
        },
    );
}

/// Ask before withdrawing an install another waiting install needs.
/// Returns whether the page opened (an undo with no dependent is immediate).
pub(super) fn open_undo_dependency(content: &mut ContentSession, entry: &Entry) -> bool {
    let Some(dir) = content
        .pending_of(&entry.dir, &entry.key)
        .filter(|p| matches!(p.change, PendingKind::Install { .. }))
        .map(|p| p.dir.clone())
    else {
        return false;
    };
    let needing = dependents(content, &entry.key, true);
    if needing.is_empty() {
        return false;
    }
    open(
        content,
        ConfirmPage {
            question: format!("Don't install {}?", cap(&entry.name, NAME_CAP)),
            body: needs_sentence(&needing, "will not load"),
            variant: Variant::Destroy,
            action_text: "Don't install",
            now_text: "",
            confirmed: Confirmed::Undo { dir },
        },
    );
    true
}

/// The copy of the exit confirm: `(question, body, when done, now)`.
pub(in crate::app) fn exit_copy(
    kind: &ExitKind,
    jobs: usize,
) -> (String, String, &'static str, &'static str) {
    let noun = if jobs == 1 { "download" } else { "downloads" };
    let (question, go, now) = match kind {
        ExitKind::Quit => ("Quit now?", "Quit when done", "Quit now"),
        ExitKind::Restart { .. } => ("Restart now?", "Restart when done", "Restart now"),
    };
    (
        question.to_owned(),
        format!("{jobs} {noun} will be cancelled."),
        go,
        now,
    )
}

pub(super) fn open_exit(content: &mut ContentSession, kind: ExitKind) {
    let (question, body, go, now) = exit_copy(&kind, content.jobs.len());
    open(
        content,
        ConfirmPage {
            question,
            body,
            variant: Variant::Exit,
            action_text: go,
            now_text: now,
            confirmed: Confirmed::Exit { kind },
        },
    );
}
