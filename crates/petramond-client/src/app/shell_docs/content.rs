mod confirm;
pub(in crate::app) mod rows;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use petramond::content::ListingRow;
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};

use super::{ScreenCtx, ShellCommand};
use crate::app::content::ListingState;
use crate::app::{App, AppScreen};
use confirm::ConfirmPage;
use rows::{Action, Entry, Local, Message, MessageAction, Slot};

const LIST: &str = "content";
const TABS: &str = "tabs";
const SEARCH_MAX_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Tab {
    Installed,
    Browse,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Installed, Tab::Browse];

    fn tip(self) -> &'static str {
        match self {
            Tab::Installed => "Installed",
            Tab::Browse => "Browse content",
        }
    }
}

pub(in crate::app) struct ContentView {
    pub(in crate::app) tab: Tab,
    pub(in crate::app) back: Option<String>,
    filter: Option<Vec<String>>,
    pub(in crate::app) locals: Vec<Local>,
    layout: Vec<Slot>,
    shown: Vec<Slot>,
    known: BTreeMap<String, ListingRow>,
    selected: Option<String>,
    expanded: BTreeSet<String>,
    search: String,
    confirm: Option<ConfirmPage>,
    rebuild_on_arrival: bool,
    needs_rebuild: bool,
    frame: u64,
    toggled: Option<(u32, u64)>,
}

impl ContentView {
    pub(in crate::app) fn new(
        back: Option<String>,
        filter: Option<Vec<String>>,
        rebuild_on_arrival: bool,
    ) -> Self {
        Self {
            tab: if filter.is_some() {
                Tab::Browse
            } else {
                Tab::Installed
            },
            back,
            filter,
            locals: Vec::new(),
            layout: Vec::new(),
            shown: Vec::new(),
            known: BTreeMap::new(),
            selected: None,
            expanded: BTreeSet::new(),
            search: String::new(),
            confirm: None,
            rebuild_on_arrival,
            needs_rebuild: true,
            frame: 0,
            toggled: None,
        }
    }

    pub(in crate::app) fn request_rebuild(&mut self) {
        self.needs_rebuild = true;
    }

    pub(in crate::app) fn listing_landed(&mut self, rows: &[ListingRow]) {
        for row in rows {
            self.known.insert(row.mod_id.clone(), row.clone());
        }
        if std::mem::take(&mut self.rebuild_on_arrival) {
            self.needs_rebuild = true;
        }
    }

    pub(in crate::app) fn show_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.selected = None;
            self.needs_rebuild = true;
        }
    }

    fn rebuild(&mut self, session: &crate::app::content::ContentSession) {
        self.needs_rebuild = false;
        for row in &session.rows {
            self.known.insert(row.mod_id.clone(), row.clone());
        }
        let mut layout = Vec::new();
        if self.filter.is_some() {
            layout.push(Slot::FilterBanner);
        }
        let local_of = |id: &str| self.locals.iter().position(|l| l.id.as_deref() == Some(id));
        match self.tab {
            Tab::Installed => {
                let mut installed: Vec<usize> = (0..self.locals.len())
                    .filter(|&i| !self.locals[i].shipped())
                    .collect();
                installed.sort_by_key(|&i| self.locals[i].name.to_lowercase());
                if installed.is_empty() {
                    layout.push(Slot::NothingInstalled);
                }
                layout.extend(installed.into_iter().map(Slot::Local));
            }
            Tab::Browse => {
                if !session.installs_enabled {
                    layout.push(Slot::InstallsOff);
                }
                layout.push(Slot::ListingStatus);
                if session.rows_known {
                    layout.extend(
                        session
                            .rows
                            .iter()
                            .filter_map(|r| match local_of(&r.mod_id) {
                                Some(i) if self.locals[i].shipped() => None,
                                Some(i) => Some(Slot::Local(i)),
                                None => Some(Slot::Listed(r.mod_id.clone())),
                            }),
                    );
                }
                if self.filter.is_some() {
                    layout.push(Slot::NotListed);
                }
            }
        }
        self.layout = layout;
    }

    fn key_of(&self, slot: &Slot) -> Option<String> {
        match slot {
            Slot::Local(i) => Some(self.locals[*i].key.clone()),
            Slot::Listed(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn slot_text(&self, slot: &Slot) -> Option<(String, String, Option<String>)> {
        match slot {
            Slot::Local(i) => {
                let l = &self.locals[*i];
                Some((l.name.clone(), l.summary.clone(), l.id.clone()))
            }
            Slot::Listed(id) => self
                .known
                .get(id)
                .map(|r| (r.name.clone(), r.summary.clone(), Some(r.mod_id.clone()))),
            _ => None,
        }
    }

    fn passes(&self, slot: &Slot) -> bool {
        let Some((name, summary, id)) = self.slot_text(slot) else {
            return true;
        };
        if let Some(filter) = &self.filter {
            if !id.as_ref().is_some_and(|id| filter.contains(id)) {
                return false;
            }
        }
        let needle = self.search.trim().to_lowercase();
        needle.is_empty()
            || name.to_lowercase().contains(&needle)
            || summary.to_lowercase().contains(&needle)
            || id.is_some_and(|id| id.contains(&needle))
    }

    fn refresh_shown(&mut self, session: &crate::app::content::ContentSession) {
        let searching = !self.search.trim().is_empty();
        let shown = self
            .layout
            .iter()
            .filter(|slot| match slot {
                Slot::FilterBanner => true,
                Slot::InstallsOff | Slot::NothingInstalled => !searching,
                Slot::ListingStatus | Slot::NotListed => {
                    !searching && self.message(slot, session).is_some()
                }
                Slot::Local(_) | Slot::Listed(_) => self.passes(slot),
            })
            .cloned()
            .collect();
        self.shown = shown;
    }

    fn available_count(&self) -> usize {
        self.layout
            .iter()
            .filter(|s| matches!(s, Slot::Listed(_) | Slot::Local(_)) && self.passes(s))
            .count()
    }

    fn message(
        &self,
        slot: &Slot,
        session: &crate::app::content::ContentSession,
    ) -> Option<Message> {
        match slot {
            Slot::NothingInstalled => Some(rows::nothing_installed_message()),
            Slot::FilterBanner => Some(rows::filter_message()),
            Slot::InstallsOff => Some(rows::installs_off_message()),
            Slot::ListingStatus => rows::listing_message(session, self.available_count()),
            Slot::NotListed => {
                let filter = self.filter.as_ref()?;
                if !session.rows_known {
                    return None;
                }
                let missing: Vec<String> = filter
                    .iter()
                    .filter(|id| {
                        !session.rows.iter().any(|r| &r.mod_id == *id)
                            && !self.locals.iter().any(|l| l.id.as_ref() == Some(id))
                    })
                    .cloned()
                    .collect();
                (!missing.is_empty()).then(|| rows::not_listed_message(&missing))
            }
            _ => None,
        }
    }

    fn entry(
        &self,
        slot: &Slot,
        session: &crate::app::content::ContentSession,
        now: Instant,
    ) -> Option<Entry> {
        let mut entry = match slot {
            Slot::Local(i) => rows::local_entry(&self.locals[*i], session, now),
            Slot::Listed(id) => rows::listed_entry(self.known.get(id)?, session, now),
            _ => return None,
        };
        if !session.installs_enabled
            && matches!(
                entry.action,
                Action::Get | Action::Update | Action::Retry | Action::Replace
            )
        {
            entry.action = Action::None;
        }
        let pack_icon = crate::app::content::pack_icon_name(&entry.key);
        entry.icon = match slot {
            Slot::Local(_)
                if crate::app::content::pack_icons()
                    .iter()
                    .any(|i| i.key == pack_icon) =>
            {
                Some(pack_icon)
            }
            _ if session.icons.has_site(&entry.key) => {
                Some(crate::app::content::site_icon_name(&entry.key))
            }
            _ => None,
        };
        Some(entry)
    }

    fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.shown
            .iter()
            .position(|s| self.key_of(s).as_ref() == Some(key))
    }
}

pub(super) fn prepare(ctx: &mut ScreenCtx) -> bool {
    if ctx.content.view.is_none() {
        ctx.goto(AppScreen::Title);
        return false;
    }
    let signed_out = matches!(ctx.content.listing, ListingState::SignedOut { .. });
    let signed_in = ctx.shell.account.saved.is_some();
    if !signed_in && !signed_out {
        ctx.content.signed_out(false);
    } else if signed_in && signed_out {
        ctx.content.fetch_listing(ctx.now);
        if let Some(view) = ctx.content.view.as_mut() {
            view.rebuild_on_arrival = true;
        }
    }
    let session = &mut *ctx.content;
    let mut view = session.view.take().expect("checked above");
    view.frame += 1;
    if view.needs_rebuild {
        view.rebuild(session);
    }
    view.refresh_shown(session);
    session.view = Some(view);
    let mut images: Vec<_> = crate::app::content::pack_icons().to_vec();
    images.extend(ctx.content.icons.site.iter().cloned());
    ctx.ui.set_dynamic_images(images);
    true
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let session = &*ctx.content;
    let Some(view) = session.view.as_ref() else {
        return;
    };
    let now = Instant::now();
    let sweep = (ctx.now - session.listing_started).rem_euclid(1.0) as f32;
    let hovered = ctx.ui.hover_item(LIST);
    let selected = view.selected_index();
    let mut bound = Vec::with_capacity(view.shown.len());
    state.set("message_tip", UiValue::Str(String::new()));
    let mut hovered_entry = None;
    for (i, slot) in view.shown.iter().enumerate() {
        let mut row = UiMap::new();
        let kind = |row: &mut UiMap, message: bool| {
            row.insert("is_message".into(), UiValue::Bool(message));
            row.insert("is_entry".into(), UiValue::Bool(!message));
        };
        match slot {
            Slot::Local(_) | Slot::Listed(_) => {
                kind(&mut row, false);
                if let Some(entry) = view.entry(slot, session, now) {
                    let expanded = view.expanded.contains(&entry.key);
                    bind_entry(&mut row, &entry, expanded);
                    if hovered == Some(i) {
                        hovered_entry = Some(entry);
                    }
                }
            }
            _ => {
                kind(&mut row, true);
                if let Some(message) = view.message(slot, session) {
                    bind_message(&mut row, &message, sweep);
                    if hovered == Some(i) {
                        state.set(
                            "message_tip",
                            UiValue::Str(
                                message.action.map(MessageAction::tip).unwrap_or("").into(),
                            ),
                        );
                    }
                }
            }
        }
        bound.push(row);
    }
    state.set("rows", UiValue::List(Arc::new(bound)));
    state.set(
        "browse_empty",
        UiValue::Bool(
            view.shown
                .iter()
                .any(|slot| matches!(slot, Slot::NothingInstalled)),
        ),
    );
    state.set(
        "tab_sel",
        UiValue::I32(Tab::ALL.iter().position(|&t| t == view.tab).unwrap_or(0) as i32),
    );
    let tab_tip = ctx
        .ui
        .hover_item(TABS)
        .and_then(|i| Tab::ALL.get(i))
        .map_or("", |t| t.tip());
    state.set("tab_tip", UiValue::Str(tab_tip.into()));
    state.set("row_sel", UiValue::I32(selected.map_or(-1, |i| i as i32)));
    state.set("search", UiValue::Str(view.search.clone()));
    bind_tips(
        state,
        hovered_entry.as_ref(),
        hovered.is_some() && hovered == selected,
    );
    state.set(
        "account_tip",
        UiValue::Str(match &ctx.shell.account.saved {
            Some(saved) => format!("Account: {}", saved.username),
            None => "Sign in".to_owned(),
        }),
    );
    state.set(
        "pending_changes",
        UiValue::Bool(!session.pending.is_empty()),
    );
    state.set(
        "apply_status",
        UiValue::Str(
            if ctx.content_report.deferred && !session.pending.is_empty() {
                "Close other Petramond windows, then apply".to_owned()
            } else if !ctx.content_report.failed.is_empty() {
                "Some changes could not be applied".to_owned()
            } else {
                String::new()
            },
        ),
    );
    state.set(
        "apply_tip",
        UiValue::Str(rows::apply_tip(session, |dir| {
            let locals = || view.locals.iter();
            locals()
                .find(|l| l.dir == dir)
                .or_else(|| locals().find(|l| l.id.as_deref() == Some(dir)))
                .map(|l| l.name.clone())
        })),
    );
    confirm::populate(view.confirm.as_ref(), state);
}

fn bind_entry(row: &mut UiMap, entry: &Entry, expanded: bool) {
    let s = |v: &str| UiValue::Str(v.to_owned());
    row.insert("name".into(), s(&entry.name));
    row.insert("is_addon".into(), UiValue::Bool(entry.is_addon));
    let version = entry
        .version
        .as_deref()
        .map(|v| format!("v{}", rows::cap(v, 12)));
    row.insert("has_version".into(), UiValue::Bool(version.is_some()));
    row.insert("version".into(), UiValue::Str(version.unwrap_or_default()));
    row.insert("summary".into(), s(&entry.summary));
    let description = if entry.description.is_empty() {
        &entry.summary
    } else {
        &entry.description
    };
    row.insert("description".into(), s(description));
    row.insert("expanded".into(), UiValue::Bool(expanded));
    row.insert("collapsed".into(), UiValue::Bool(!expanded));
    let note = entry.world_note.filter(|_| expanded);
    row.insert("has_world_note".into(), UiValue::Bool(note.is_some()));
    row.insert("world_note".into(), s(note.unwrap_or("")));
    row.insert("detail".into(), s(&entry.detail));
    row.insert("detail_palette".into(), s(entry.detail_palette));
    row.insert("has_icon".into(), UiValue::Bool(entry.icon.is_some()));
    row.insert("no_icon".into(), UiValue::Bool(entry.icon.is_none()));
    row.insert("icon".into(), s(entry.icon.as_deref().unwrap_or("")));
    let (get, text) = match entry.action {
        Action::Get => (true, "Get"),
        Action::Update => (true, "Update"),
        Action::Retry => (true, "Retry"),
        _ => (false, ""),
    };
    row.insert("can_get".into(), UiValue::Bool(get));
    row.insert("action_text".into(), s(text));
    row.insert(
        "can_replace".into(),
        UiValue::Bool(entry.action == Action::Replace),
    );
    row.insert(
        "can_undo".into(),
        UiValue::Bool(entry.action == Action::Undo),
    );
    let busy = matches!(entry.action, Action::Busy(_));
    row.insert("busy".into(), UiValue::Bool(busy));
    let progress = match entry.action {
        Action::Busy(fraction) => fraction,
        _ => 0.0,
    };
    row.insert("progress".into(), UiValue::F32(progress));
    row.insert("can_delete".into(), UiValue::Bool(entry.can_delete));
}

fn bind_message(row: &mut UiMap, message: &Message, sweep: f32) {
    row.insert("message".into(), UiValue::Str(message.text.clone()));
    row.insert(
        "message_palette".into(),
        UiValue::Str(message.palette.into()),
    );
    row.insert("message_busy".into(), UiValue::Bool(message.busy));
    row.insert("message_progress".into(), UiValue::F32(sweep));
    row.insert(
        "has_message_action".into(),
        UiValue::Bool(
            message
                .action
                .is_some_and(|action| action != MessageAction::Browse),
        ),
    );
    row.insert(
        "message_action_text".into(),
        UiValue::Str(message.action.map(|a| a.text()).unwrap_or("").into()),
    );
}

fn bind_tips(state: &mut UiState, entry: Option<&Entry>, selected: bool) {
    let hint = |tip: &str, key: &str| {
        if tip.is_empty() || !selected {
            tip.to_owned()
        } else {
            format!("{tip} ({key})")
        }
    };
    let tip = |state: &mut UiState, key: &str, value: String| state.set(key, UiValue::Str(value));
    let Some(entry) = entry else {
        for key in [
            "replace_tip",
            "undo_tip",
            "delete_tip",
            "cancel_tip",
            "detail_tip",
        ] {
            tip(state, key, String::new());
        }
        return;
    };
    let action = |on: bool| if on { entry.action_tip.as_str() } else { "" };
    tip(
        state,
        "replace_tip",
        hint(action(entry.action == Action::Replace), "Enter"),
    );
    tip(
        state,
        "undo_tip",
        hint(action(entry.action == Action::Undo), "Ctrl+Z"),
    );
    let delete = match entry.can_delete {
        true => format!("Delete {}", rows::cap(&entry.name, rows::NAME_CAP)),
        false => String::new(),
    };
    tip(state, "delete_tip", hint(&delete, "Del"));
    tip(
        state,
        "cancel_tip",
        hint(
            if matches!(entry.action, Action::Busy(_)) {
                "Cancel download"
            } else {
                ""
            },
            "Del",
        ),
    );
    tip(state, "detail_tip", entry.detail_tip.clone());
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if ctx
        .content
        .view
        .as_ref()
        .is_some_and(|v| v.confirm.is_some())
    {
        return confirm::handle(ctx, ev);
    }
    match ev {
        UiEvent::TextChanged { id, text } if id == "search" => {
            if let Some(view) = ctx.content.view.as_mut() {
                view.search = text;
            }
        }
        UiEvent::Submit { id, .. } if id == "search" => primary(ctx),
        UiEvent::TabSelect { id, index } if id == TABS => {
            if let (Some(view), Some(&tab)) =
                (ctx.content.view.as_mut(), Tab::ALL.get(index as usize))
            {
                view.show_tab(tab);
            }
        }
        UiEvent::ListSelect { index, .. } => select_row(ctx, index, true),
        UiEvent::ListActivate { index, .. } => {
            if let Some(view) = ctx.content.view.as_mut() {
                if view.toggled == Some((index, view.frame)) {
                    if let Some(key) = view.shown.get(index as usize).and_then(|s| view.key_of(s)) {
                        toggle(&mut view.expanded, &key);
                    }
                }
            }
            primary(ctx);
        }
        UiEvent::Click { id, item, .. } => {
            if let Some(index) = item {
                select_row(ctx, index, false);
            }
            click(ctx, &id, item);
        }
        UiEvent::Key { key, ctrl, .. } => self::key(ctx, key, ctrl),
        _ => {}
    }
}

fn toggle(set: &mut BTreeSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_owned());
    }
}

fn select_row(ctx: &mut ScreenCtx, index: u32, toggle_expansion: bool) {
    let Some(view) = ctx.content.view.as_mut() else {
        return;
    };
    let Some(key) = view.shown.get(index as usize).and_then(|s| view.key_of(s)) else {
        return;
    };
    if toggle_expansion {
        toggle(&mut view.expanded, &key);
        view.toggled = Some((index, view.frame));
    }
    view.selected = Some(key);
}

fn click(ctx: &mut ScreenCtx, id: &str, item: Option<u32>) {
    match id {
        "back" => ctx.request(ShellCommand::CloseContent),
        "account" => ctx.request(ShellCommand::OpenAccount(None)),
        "browse_empty" => {
            if let Some(view) = ctx.content.view.as_mut() {
                view.show_tab(Tab::Browse);
            }
        }
        "refresh" => refresh(ctx),
        "apply" => ctx.request(ShellCommand::ApplyContent),
        "get" | "replace" => primary(ctx),
        "undo" => undo(ctx),
        "delete" | "cancel" => trash(ctx),
        "message_action" => {
            let slot = ctx
                .content
                .view
                .as_ref()
                .and_then(|v| item.and_then(|i| v.shown.get(i as usize)).cloned());
            message_action(ctx, slot);
        }
        _ => {}
    }
}

fn message_action(ctx: &mut ScreenCtx, slot: Option<Slot>) {
    let Some(slot) = slot else { return };
    let Some(action) = ctx
        .content
        .view
        .as_ref()
        .and_then(|v| v.message(&slot, ctx.content))
        .and_then(|m| m.action)
    else {
        return;
    };
    match action {
        MessageAction::SignIn => ctx.request(ShellCommand::OpenAccountSignIn),
        MessageAction::CancelLoad => ctx.content.cancel_listing(),
        MessageAction::Retry => refresh(ctx),
        MessageAction::Browse => {
            if let Some(view) = ctx.content.view.as_mut() {
                view.show_tab(Tab::Browse);
            }
        }
        MessageAction::ShowAll => {
            if let Some(view) = ctx.content.view.as_mut() {
                view.filter = None;
                view.request_rebuild();
            }
        }
    }
}

fn key(ctx: &mut ScreenCtx, key: NavKey, ctrl: bool) {
    let searching = ctx.ui.text_input_focused();
    match key {
        NavKey::F(5) => refresh(ctx),
        NavKey::Char('f') if ctrl => {
            let text = ctx
                .content
                .view
                .as_ref()
                .map(|v| v.search.clone())
                .unwrap_or_default();
            ctx.ui.focus_text_input("search", &text, SEARCH_MAX_CHARS);
            ctx.ui.push_input(petramond_ui::InputEvent::Key {
                key: NavKey::SelectAll,
                shift: false,
                ctrl: false,
            });
        }
        NavKey::Char('z') if ctrl => undo(ctx),
        NavKey::Up | NavKey::Down => {
            if searching {
                ctx.ui.push_input(petramond_ui::InputEvent::Key {
                    key: NavKey::Escape,
                    shift: false,
                    ctrl: false,
                });
            }
            move_selection(ctx, if key == NavKey::Up { -1 } else { 1 });
        }
        NavKey::Right | NavKey::Left if !searching => {
            if let Some(view) = ctx.content.view.as_mut() {
                if let Some(selected) = view.selected.clone() {
                    if key == NavKey::Right {
                        view.expanded.insert(selected);
                    } else {
                        view.expanded.remove(&selected);
                    }
                }
            }
        }
        NavKey::Enter => primary(ctx),
        NavKey::Delete if !searching => trash(ctx),
        _ => {}
    }
}

fn refresh(ctx: &mut ScreenCtx) {
    if ctx.shell.account.saved.is_none() {
        return;
    }
    let now = ctx.now;
    ctx.content.failed.clear();
    ctx.content.fetch_listing(now);
    if let Some(view) = ctx.content.view.as_mut() {
        view.rebuild_on_arrival = true;
        view.request_rebuild();
    }
}

fn selected_entry(ctx: &ScreenCtx) -> Option<Entry> {
    let view = ctx.content.view.as_ref()?;
    let slot = view.shown.get(view.selected_index()?)?;
    view.entry(slot, ctx.content, Instant::now())
}

fn primary(ctx: &mut ScreenCtx) {
    let Some(entry) = selected_entry(ctx) else {
        return;
    };
    if !entry.action.is_primary() {
        return;
    }
    let Some(row) = entry.listed.clone() else {
        return;
    };
    if entry.action == Action::Replace {
        return confirm::open_replace(ctx.content, &entry, row);
    }
    if let Err(why) = ctx.content.get(&row) {
        ctx.content.failed.insert(row.mod_id.clone(), why);
    }
}

fn undo(ctx: &mut ScreenCtx) {
    let Some(entry) = selected_entry(ctx) else {
        return;
    };
    if entry.action != Action::Undo {
        return;
    }
    if confirm::open_undo_dependency(ctx.content, &entry) {
        return;
    }
    if let Some(dir) = ctx
        .content
        .pending_of(&entry.dir, &entry.key)
        .map(|p| p.dir.clone())
    {
        ctx.content.undo(&dir);
    }
}

fn trash(ctx: &mut ScreenCtx) {
    let Some(entry) = selected_entry(ctx) else {
        return;
    };
    if matches!(entry.action, Action::Busy(_)) {
        ctx.content.jobs.cancel(&entry.key);
        ctx.content.exit_when_idle = None;
    } else if entry.can_delete {
        confirm::open_delete(ctx.content, &entry);
    }
}

fn move_selection(ctx: &mut ScreenCtx, step: i32) {
    let Some(view) = ctx.content.view.as_mut() else {
        return;
    };
    let entries: Vec<usize> = (0..view.shown.len())
        .filter(|&i| view.key_of(&view.shown[i]).is_some())
        .collect();
    if entries.is_empty() {
        return;
    }
    let at = view
        .selected_index()
        .and_then(|i| entries.iter().position(|&e| e == i));
    let next = match at {
        None if step > 0 => 0,
        None => entries.len() - 1,
        Some(at) => (at as i32 + step).clamp(0, entries.len() as i32 - 1) as usize,
    };
    view.selected = view.key_of(&view.shown[entries[next]]);
}

impl App {
    pub(in crate::app) fn content_escape(&mut self) {
        if let Some(view) = self.content.view.as_mut() {
            if view.confirm.take().is_some() {
                return;
            }
        }
        if !self.ui.text_input_focused() {
            self.close_content();
        }
    }

    pub(in crate::app) fn confirm_content_exit(&mut self, kind: crate::app::ExitKind) {
        confirm::open_exit(&mut self.content, kind);
    }
}

#[cfg(test)]
mod tests;
