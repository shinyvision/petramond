use super::text_input::{TextClipboard, TextInput};
use super::{now_seconds, App, AppScreen, HandTriggers};
use crate::camera::Camera;
use crate::controls::{text_shortcut_from_key_code, TextKey, TextShortcut};
use crate::gui::{
    gui_scale, shell_def, shell_input_visible_chars, ShellButton, ShellDef, ShellInput, ShellKind,
    ShellListRow, ShellQuad, ShellRole, ShellScrollbar, ShellText, ShellTextAlign, ShellUiSnapshot,
    SlotRect,
};
use crate::mathh::Vec3;
use crate::save::WorldInfo;

const DOUBLE_CLICK_SECS: f64 = 0.25;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ShellButtonId {
    TitleStart,
    TitleQuit,
    WorldPlay,
    WorldCreate,
    WorldDelete,
    WorldBack,
    CreateCreate,
    CreateCancel,
    DeleteWorldConfirm,
    DeleteWorldCancel,
    PauseResume,
    PauseSaveQuit,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CreateField {
    Name,
    Seed,
}

#[derive(Default)]
pub(super) struct ShellClickStreak {
    world: Option<usize>,
    time: f64,
}

impl ShellClickStreak {
    fn register_world(&mut self, world: usize, now: f64) -> bool {
        let double = self.world == Some(world) && now - self.time < DOUBLE_CLICK_SECS;
        if double {
            self.world = None;
        } else {
            self.world = Some(world);
            self.time = now;
        }
        double
    }

    pub(super) fn reset(&mut self) {
        self.world = None;
    }
}

struct ShellLayout {
    snapshot: ShellUiSnapshot,
    buttons: Vec<(ShellButtonId, SlotRect, bool)>,
    inputs: Vec<ShellInputHit>,
    rows: Vec<(usize, SlotRect)>,
    scroll_tracks: Vec<ShellScrollTrack>,
}

#[derive(Copy, Clone)]
struct ShellInputHit {
    field: CreateField,
    rect: SlotRect,
    scale: f32,
    visible_chars: usize,
}

struct ShellScrollTrack {
    track: SlotRect,
    total: usize,
    visible: usize,
}

impl App {
    pub(super) fn refresh_worlds(&mut self) {
        self.worlds = match crate::save::list_worlds() {
            Ok(worlds) => worlds,
            Err(e) => {
                log::warn!("could not list worlds: {e}");
                Vec::new()
            }
        };
        if let Some(selected) = self.selected_world {
            if selected >= self.worlds.len() {
                self.selected_world = None;
            }
        }
        self.clamp_world_scroll();
    }

    pub(super) fn shell_ui_snapshot(
        &self,
        screen_size: (u32, u32),
        cursor: (f32, f32),
    ) -> ShellUiSnapshot {
        self.shell_layout(screen_size, cursor, now_seconds())
            .snapshot
    }

    pub(super) fn route_shell_click(&mut self, screen_size: (u32, u32), now: f64) -> bool {
        if !self.screen.shell_open() {
            return false;
        }

        let cursor = self.pointer.cursor();
        let layout = self.shell_layout(screen_size, cursor, now);
        for input in layout.inputs {
            if input.rect.contains(cursor.0, cursor.1) {
                self.focus_create_field(input.field, now);
                let index = self.create_input(input.field).cursor_index_for_x(
                    cursor.0,
                    input.rect,
                    input.scale,
                    input.visible_chars,
                );
                let anchor =
                    self.create_input_mut(input.field)
                        .begin_drag(index, input.visible_chars, now);
                self.dragged_create_field = Some((input.field, anchor));
                return true;
            }
        }

        for (world, rect) in layout.rows {
            if rect.contains(cursor.0, cursor.1) {
                let double = self.shell_clicks.register_world(world, now);
                self.selected_world = Some(world);
                if double {
                    self.play_selected_world();
                }
                return true;
            }
        }

        for scroll in layout.scroll_tracks {
            if scroll.track.contains(cursor.0, cursor.1) {
                self.world_scroll =
                    scroll_from_track_click(cursor.1, scroll.track, scroll.total, scroll.visible);
                self.shell_clicks.reset();
                return true;
            }
        }

        for (id, rect, enabled) in layout.buttons {
            if rect.contains(cursor.0, cursor.1) {
                if enabled {
                    self.apply_shell_button(id);
                }
                return true;
            }
        }

        if matches!(self.screen, AppScreen::CreateWorld) {
            self.clear_create_focus();
        }
        true
    }

    pub(super) fn route_shell_drag(&mut self, screen_size: (u32, u32), now: f64) -> bool {
        let Some((field, anchor)) = self.dragged_create_field else {
            return false;
        };
        if !matches!(self.screen, AppScreen::CreateWorld) {
            self.dragged_create_field = None;
            return false;
        }

        let cursor = self.pointer.cursor();
        let layout = self.shell_layout(screen_size, cursor, now);
        let Some(input) = layout.inputs.into_iter().find(|input| input.field == field) else {
            self.dragged_create_field = None;
            return false;
        };
        let index = self.create_input(field).cursor_index_for_x(
            cursor.0,
            input.rect,
            input.scale,
            input.visible_chars,
        );
        self.create_input_mut(field)
            .drag_to(anchor, index, input.visible_chars, now);
        true
    }

    pub(super) fn clear_shell_drag(&mut self) {
        self.dragged_create_field = None;
    }

    pub(super) fn shell_text_cursor_hovered(&self, screen_size: (u32, u32)) -> bool {
        if !matches!(self.screen, AppScreen::CreateWorld) {
            return false;
        }
        if self.dragged_create_field.is_some() {
            return true;
        }
        let cursor = self.pointer.cursor();
        self.shell_layout(screen_size, cursor, now_seconds())
            .inputs
            .into_iter()
            .any(|input| input.rect.contains(cursor.0, cursor.1))
    }

    pub fn handle_text_key(&mut self, key: TextKey, screen_size: (u32, u32)) -> bool {
        match self.screen {
            AppScreen::CreateWorld => self.handle_create_world_key(key, screen_size),
            AppScreen::WorldSelect => self.handle_world_select_key(key),
            AppScreen::DeleteWorld => self.handle_delete_world_key(key),
            AppScreen::Pause => {
                if matches!(key, TextKey::Enter) {
                    self.resume_game();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn handle_text_shortcut_code(
        &mut self,
        code: winit::keyboard::KeyCode,
        clipboard: &mut dyn TextClipboard,
        screen_size: (u32, u32),
    ) -> bool {
        let Some(shortcut) = text_shortcut_from_key_code(code, self.modifiers) else {
            return false;
        };
        self.handle_text_shortcut(shortcut, clipboard, screen_size)
    }

    pub fn handle_text_shortcut(
        &mut self,
        shortcut: TextShortcut,
        clipboard: &mut dyn TextClipboard,
        screen_size: (u32, u32),
    ) -> bool {
        if !matches!(self.screen, AppScreen::CreateWorld) {
            return false;
        }
        let Some(field) = self.focused_create_field else {
            return false;
        };
        let visible_chars = self.create_field_visible_chars(field, screen_size);
        let now = now_seconds();
        match shortcut {
            TextShortcut::SelectAll => {
                self.create_input_mut(field).select_all(visible_chars, now);
                true
            }
            TextShortcut::Cut => {
                self.create_input_mut(field)
                    .cut_selection(clipboard, visible_chars, now);
                true
            }
            TextShortcut::Copy => {
                self.create_input(field).copy_selection(clipboard);
                true
            }
            TextShortcut::Paste => {
                self.create_input_mut(field)
                    .paste(clipboard, visible_chars, now);
                true
            }
        }
    }

    pub fn handle_text_input(&mut self, text: &str, screen_size: (u32, u32)) -> bool {
        if !matches!(self.screen, AppScreen::CreateWorld) {
            return false;
        }
        let Some(field) = self.focused_create_field else {
            return false;
        };
        let visible_chars = self.create_field_visible_chars(field, screen_size);
        let now = now_seconds();
        self.create_input_mut(field)
            .insert_text(text, visible_chars, now)
    }

    pub fn take_quit_requested(&mut self) -> bool {
        std::mem::take(&mut self.quit_requested)
    }

    pub(super) fn open_pause(&mut self) {
        if self.game.is_none() {
            return;
        }
        self.screen = AppScreen::Pause;
        self.pointer.release_for_menu();
        self.shell_clicks.reset();
        self.audio.set_loop(None, now_seconds());
    }

    pub(super) fn resume_game(&mut self) {
        if self.game.is_none() {
            self.screen = AppScreen::Title;
            self.pointer.release_for_menu();
            return;
        }
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
        self.shell_clicks.reset();
    }

    pub(super) fn save_and_quit_to_title(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.save_all();
        }
        self.game = None;
        self.screen = AppScreen::Title;
        self.pointer.release_for_menu();
        self.audio.set_loop(None, now_seconds());
        self.scene.clear();
        self.hand = HandTriggers::default();
        self.renderer_world_clear_pending = true;
        self.refresh_worlds();
    }

    pub(super) fn adjust_world_scroll(&mut self, delta: f32) -> bool {
        if !matches!(self.screen, AppScreen::WorldSelect) || self.worlds.is_empty() {
            return false;
        }
        let step = delta.round() as i32;
        if step == 0 {
            return true;
        }
        let visible = self.visible_world_rows_for_screen();
        let max_scroll = max_world_scroll(self.worlds.len(), visible);
        if step > 0 {
            self.world_scroll = self
                .world_scroll
                .saturating_add(step as usize)
                .min(max_scroll);
        } else {
            self.world_scroll = self.world_scroll.saturating_sub((-step) as usize);
        }
        true
    }

    fn apply_shell_button(&mut self, id: ShellButtonId) {
        match id {
            ShellButtonId::TitleStart => {
                self.refresh_worlds();
                self.selected_world = None;
                self.world_scroll = 0;
                self.screen = AppScreen::WorldSelect;
                self.pointer.release_for_menu();
            }
            ShellButtonId::TitleQuit => {
                self.quit_requested = true;
            }
            ShellButtonId::WorldPlay => self.play_selected_world(),
            ShellButtonId::WorldCreate => {
                let now = now_seconds();
                self.create_world_name.clear(now);
                self.create_world_seed.clear(now);
                self.focused_create_field = Some(CreateField::Name);
                self.create_world_name.focus(now);
                self.screen = AppScreen::CreateWorld;
                self.pointer.release_for_menu();
            }
            ShellButtonId::WorldDelete => self.open_delete_world_confirm(),
            ShellButtonId::WorldBack => {
                self.screen = AppScreen::Title;
                self.pointer.release_for_menu();
            }
            ShellButtonId::CreateCreate => self.create_world_from_form(),
            ShellButtonId::CreateCancel => {
                self.screen = AppScreen::WorldSelect;
                self.clear_create_focus();
                self.pointer.release_for_menu();
            }
            ShellButtonId::DeleteWorldConfirm => self.delete_selected_world(),
            ShellButtonId::DeleteWorldCancel => {
                self.screen = AppScreen::WorldSelect;
                self.pointer.release_for_menu();
            }
            ShellButtonId::PauseResume => self.resume_game(),
            ShellButtonId::PauseSaveQuit => self.save_and_quit_to_title(),
        }
        self.shell_clicks.reset();
    }

    fn play_selected_world(&mut self) {
        let Some(index) = self.selected_world else {
            return;
        };
        let Some(world) = self.worlds.get(index).cloned() else {
            return;
        };
        let seed = crate::save::random_seed();
        self.start_game(&world.name, seed);
    }

    fn open_delete_world_confirm(&mut self) {
        if self
            .selected_world
            .and_then(|index| self.worlds.get(index))
            .is_none()
        {
            return;
        }
        self.screen = AppScreen::DeleteWorld;
        self.pointer.release_for_menu();
    }

    fn delete_selected_world(&mut self) {
        let Some(world) = self
            .selected_world
            .and_then(|index| self.worlds.get(index))
            .cloned()
        else {
            self.screen = AppScreen::WorldSelect;
            self.pointer.release_for_menu();
            return;
        };
        if let Err(e) = crate::save::delete_world(&world.dir_name) {
            log::warn!("could not delete world '{}': {e}", world.name);
        }
        self.selected_world = None;
        self.world_scroll = 0;
        self.screen = AppScreen::WorldSelect;
        self.pointer.release_for_menu();
        self.refresh_worlds();
    }

    fn create_world_from_form(&mut self) {
        let name = self.create_world_name.text().trim().to_string();
        if !self.can_create_world() {
            return;
        }
        if let Err(e) = crate::save::write_world_metadata(&name) {
            log::warn!("could not write world metadata for '{name}': {e}");
        }
        let seed = if self.create_world_seed.text().trim().is_empty() {
            crate::save::random_seed()
        } else {
            crate::save::seed_from_text(self.create_world_seed.text())
        };
        self.start_game(&name, seed);
    }

    pub(crate) fn start_game(&mut self, world_name: &str, seed: u32) {
        let cam = Camera::new(
            Vec3::new(8.0, 90.0, 8.0),
            self.shell_camera.aspect.max(0.01),
        );
        self.game = Some(crate::game::Game::new(
            cam,
            world_name,
            seed,
            self.render_dist,
        ));
        self.clear_create_focus();
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
        self.gui_router.reset_click_streak();
        self.shell_clicks.reset();
        self.hand = HandTriggers::default();
        self.renderer_world_clear_pending = false;
    }

    fn can_create_world(&self) -> bool {
        let name = self.create_world_name.text().trim();
        !name.is_empty() && !crate::save::world_exists(name)
    }

    fn handle_create_world_key(&mut self, key: TextKey, screen_size: (u32, u32)) -> bool {
        let now = now_seconds();
        match key {
            TextKey::Backspace => {
                if let Some(field) = self.focused_create_field {
                    let visible_chars = self.create_field_visible_chars(field, screen_size);
                    self.create_input_mut(field).backspace(visible_chars, now);
                }
                true
            }
            TextKey::Delete => {
                if let Some(field) = self.focused_create_field {
                    let visible_chars = self.create_field_visible_chars(field, screen_size);
                    self.create_input_mut(field)
                        .delete_forward(visible_chars, now);
                }
                true
            }
            TextKey::Tab => {
                let field = match self.focused_create_field {
                    Some(CreateField::Name) => CreateField::Seed,
                    _ => CreateField::Name,
                };
                self.focus_create_field(field, now);
                true
            }
            TextKey::Enter => {
                self.create_world_from_form();
                true
            }
            TextKey::ArrowLeft => {
                if let Some(field) = self.focused_create_field {
                    let visible_chars = self.create_field_visible_chars(field, screen_size);
                    let shift = self.modifiers.shift;
                    self.create_input_mut(field)
                        .move_left(shift, visible_chars, now);
                    true
                } else {
                    false
                }
            }
            TextKey::ArrowRight => {
                if let Some(field) = self.focused_create_field {
                    let visible_chars = self.create_field_visible_chars(field, screen_size);
                    let shift = self.modifiers.shift;
                    self.create_input_mut(field)
                        .move_right(shift, visible_chars, now);
                    true
                } else {
                    false
                }
            }
            TextKey::ArrowUp | TextKey::ArrowDown => false,
        }
    }

    fn handle_world_select_key(&mut self, key: TextKey) -> bool {
        match key {
            TextKey::Enter => {
                self.play_selected_world();
                true
            }
            TextKey::Delete => {
                self.open_delete_world_confirm();
                true
            }
            TextKey::ArrowUp => {
                self.move_world_selection(-1);
                true
            }
            TextKey::ArrowDown => {
                self.move_world_selection(1);
                true
            }
            _ => false,
        }
    }

    fn move_world_selection(&mut self, step: i32) {
        if self.worlds.is_empty() {
            self.selected_world = None;
            return;
        }
        let current = self.selected_world.unwrap_or(0) as i32;
        let next = (current + step).clamp(0, self.worlds.len() as i32 - 1) as usize;
        self.selected_world = Some(next);
        if next < self.world_scroll {
            self.world_scroll = next;
        } else {
            let visible = self.visible_world_rows_for_screen();
            if visible > 0 && next >= self.world_scroll + visible {
                self.world_scroll = next + 1 - visible;
            }
        }
        self.clamp_world_scroll();
    }

    fn handle_delete_world_key(&mut self, key: TextKey) -> bool {
        if matches!(key, TextKey::Enter) {
            self.delete_selected_world();
            true
        } else {
            false
        }
    }

    fn clamp_world_scroll(&mut self) {
        let visible = self.visible_world_rows_for_screen();
        self.world_scroll = self
            .world_scroll
            .min(max_world_scroll(self.worlds.len(), visible));
    }

    fn visible_world_rows_for_screen(&self) -> usize {
        required_shell_def(ShellKind::WorldSelect)
            .role_rects(ShellRole::WorldRow, (1280, 720))
            .len()
            .max(1)
    }

    fn shell_layout(&self, screen: (u32, u32), cursor: (f32, f32), now: f64) -> ShellLayout {
        match self.screen {
            AppScreen::Title => title_layout(screen, cursor),
            AppScreen::WorldSelect => world_select_layout(
                screen,
                cursor,
                &self.worlds,
                self.selected_world,
                self.world_scroll,
            ),
            AppScreen::CreateWorld => create_world_layout(
                screen,
                cursor,
                &self.create_world_name,
                &self.create_world_seed,
                self.focused_create_field,
                self.can_create_world(),
                crate::save::world_exists(self.create_world_name.text().trim()),
                now,
            ),
            AppScreen::DeleteWorld => delete_world_layout(
                screen,
                cursor,
                self.selected_world.and_then(|index| self.worlds.get(index)),
            ),
            AppScreen::Pause => pause_layout(screen, cursor),
            _ => empty_layout(),
        }
    }

    fn create_input(&self, field: CreateField) -> &TextInput {
        match field {
            CreateField::Name => &self.create_world_name,
            CreateField::Seed => &self.create_world_seed,
        }
    }

    fn create_input_mut(&mut self, field: CreateField) -> &mut TextInput {
        match field {
            CreateField::Name => &mut self.create_world_name,
            CreateField::Seed => &mut self.create_world_seed,
        }
    }

    fn focus_create_field(&mut self, field: CreateField, now: f64) {
        if self.focused_create_field != Some(field) {
            if let Some(old) = self.focused_create_field {
                self.create_input_mut(old).blur();
            }
        }
        self.focused_create_field = Some(field);
        self.create_input_mut(field).focus(now);
    }

    pub(super) fn clear_create_focus(&mut self) {
        if let Some(field) = self.focused_create_field.take() {
            self.create_input_mut(field).blur();
        }
        self.dragged_create_field = None;
    }

    fn create_field_visible_chars(&self, field: CreateField, screen: (u32, u32)) -> usize {
        let def = required_shell_def(ShellKind::CreateWorld);
        let s = gui_scale(screen);
        let panel = def.panel_rect(screen);
        let (name_rect, seed_rect) = create_input_rects(screen, panel, s, def);
        let rect = match field {
            CreateField::Name => name_rect,
            CreateField::Seed => seed_rect,
        };
        shell_input_visible_chars(rect, s)
    }
}

fn empty_layout() -> ShellLayout {
    ShellLayout {
        snapshot: ShellUiSnapshot::default(),
        buttons: Vec::new(),
        inputs: Vec::new(),
        rows: Vec::new(),
        scroll_tracks: Vec::new(),
    }
}

fn title_layout(screen: (u32, u32), cursor: (f32, f32)) -> ShellLayout {
    title_skin_layout(screen, cursor, required_shell_def(ShellKind::Title))
}

fn title_skin_layout(screen: (u32, u32), cursor: (f32, f32), def: &ShellDef) -> ShellLayout {
    let s = gui_scale(screen);
    let mut layout = skin_layout(ShellKind::Title);
    let panel = def.panel_rect(screen);
    layout.snapshot.texts.push(text(
        rect(panel.x, panel.y + 16.0 * s, panel.w, 24.0 * s),
        "Llamacraft",
        3.0 * s,
        ShellTextAlign::Center,
    ));
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::TitleStart,
        ShellButtonId::TitleStart,
        "Start Game",
        true,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::TitleQuit,
        ShellButtonId::TitleQuit,
        "Quit",
        true,
        cursor,
    );
    layout
}

fn world_select_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    worlds: &[WorldInfo],
    selected: Option<usize>,
    scroll: usize,
) -> ShellLayout {
    world_select_skin_layout(
        screen,
        cursor,
        worlds,
        selected,
        scroll,
        required_shell_def(ShellKind::WorldSelect),
    )
}

fn world_select_skin_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    worlds: &[WorldInfo],
    selected: Option<usize>,
    scroll: usize,
    def: &ShellDef,
) -> ShellLayout {
    let s = gui_scale(screen);
    let mut layout = skin_layout(ShellKind::WorldSelect);
    let has_selection = selected.is_some_and(|index| index < worlds.len());
    let panel = def.panel_rect(screen);
    layout.snapshot.texts.push(text(
        rect(panel.x, panel.y + 10.0 * s, panel.w, 12.0 * s),
        "Select World",
        1.3 * s,
        ShellTextAlign::Center,
    ));

    let rows = def.role_rects(ShellRole::WorldRow, screen);
    let visible = rows.len().max(1);
    let start = scroll.min(max_world_scroll(worlds.len(), visible));
    for (row_i, row) in rows.iter().copied().enumerate() {
        let Some((world_i, world)) = worlds.iter().enumerate().skip(start).nth(row_i) else {
            continue;
        };
        layout.snapshot.rows.push(ShellListRow {
            rect: row,
            label: if world.has_level {
                world.name.clone()
            } else {
                format!("{} (new)", world.name)
            },
            selected: selected == Some(world_i),
            hovered: row.contains(cursor.0, cursor.1),
        });
        layout.rows.push((world_i, row));
    }
    if worlds.is_empty() {
        let r = rows_bounds(&rows).unwrap_or(panel);
        layout
            .snapshot
            .texts
            .push(text(r, "No worlds yet", s, ShellTextAlign::Center));
    }
    if let Some(track) = def.role_rect(ShellRole::WorldScrollTrack, screen) {
        if let Some(thumb) = scrollbar_thumb_rect(track, worlds.len(), visible, start) {
            layout
                .snapshot
                .scrollbars
                .push(ShellScrollbar { track, thumb });
            layout.scroll_tracks.push(ShellScrollTrack {
                track,
                total: worlds.len(),
                visible,
            });
        }
    }

    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::WorldPlay,
        ShellButtonId::WorldPlay,
        "Play",
        has_selection,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::WorldCreate,
        ShellButtonId::WorldCreate,
        "Create New World",
        true,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::WorldDelete,
        ShellButtonId::WorldDelete,
        "Delete World",
        has_selection,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::WorldBack,
        ShellButtonId::WorldBack,
        "Back",
        true,
        cursor,
    );
    layout
}

fn delete_world_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    world: Option<&WorldInfo>,
) -> ShellLayout {
    delete_world_skin_layout(
        screen,
        cursor,
        world,
        required_shell_def(ShellKind::DeleteWorld),
    )
}

fn delete_world_skin_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    world: Option<&WorldInfo>,
    def: &ShellDef,
) -> ShellLayout {
    let s = gui_scale(screen);
    let mut layout = skin_layout(ShellKind::DeleteWorld);
    let panel = def.panel_rect(screen);
    layout.snapshot.texts.push(text(
        rect(panel.x, panel.y + 14.0 * s, panel.w, 12.0 * s),
        "Delete World",
        1.25 * s,
        ShellTextAlign::Center,
    ));
    layout.snapshot.texts.push(text(
        rect(
            panel.x + 16.0 * s,
            panel.y + 44.0 * s,
            panel.w - 32.0 * s,
            12.0 * s,
        ),
        if world.is_some() {
            "Delete this world?"
        } else {
            "No world selected"
        },
        s,
        ShellTextAlign::Center,
    ));
    if let Some(world) = world {
        layout.snapshot.texts.push(text(
            rect(
                panel.x + 16.0 * s,
                panel.y + 62.0 * s,
                panel.w - 32.0 * s,
                12.0 * s,
            ),
            &world.name,
            s,
            ShellTextAlign::Center,
        ));
    }
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::DeleteWorldConfirm,
        ShellButtonId::DeleteWorldConfirm,
        "Delete World",
        world.is_some(),
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::DeleteWorldCancel,
        ShellButtonId::DeleteWorldCancel,
        "Cancel",
        true,
        cursor,
    );
    layout
}

fn create_world_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    name: &TextInput,
    seed: &TextInput,
    focused: Option<CreateField>,
    can_create: bool,
    duplicate: bool,
    now: f64,
) -> ShellLayout {
    create_world_skin_layout(
        screen,
        cursor,
        name,
        seed,
        focused,
        can_create,
        duplicate,
        now,
        required_shell_def(ShellKind::CreateWorld),
    )
}

fn create_world_skin_layout(
    screen: (u32, u32),
    cursor: (f32, f32),
    name: &TextInput,
    seed: &TextInput,
    focused: Option<CreateField>,
    can_create: bool,
    duplicate: bool,
    now: f64,
    def: &ShellDef,
) -> ShellLayout {
    let s = gui_scale(screen);
    let mut layout = skin_layout(ShellKind::CreateWorld);
    let panel = def.panel_rect(screen);
    layout.snapshot.texts.push(text(
        rect(panel.x, panel.y + 10.0 * s, panel.w, 12.0 * s),
        "Create New World",
        1.25 * s,
        ShellTextAlign::Center,
    ));

    let (name_rect, seed_rect) = create_input_rects(screen, panel, s, def);
    layout.snapshot.texts.push(text(
        rect(name_rect.x, name_rect.y - 12.0 * s, name_rect.w, 10.0 * s),
        "World Name",
        s,
        ShellTextAlign::Left,
    ));
    layout.snapshot.texts.push(text(
        rect(seed_rect.x, seed_rect.y - 12.0 * s, seed_rect.w, 10.0 * s),
        "Seed",
        s,
        ShellTextAlign::Left,
    ));
    push_input(
        &mut layout,
        CreateField::Name,
        name_rect,
        name,
        "My World",
        focused == Some(CreateField::Name),
        s,
        now,
    );
    push_input(
        &mut layout,
        CreateField::Seed,
        seed_rect,
        seed,
        "Blank = random",
        focused == Some(CreateField::Seed),
        s,
        now,
    );
    if duplicate && !name.text().trim().is_empty() {
        layout.snapshot.texts.push(ShellText {
            rect: rect(seed_rect.x, seed_rect.y + 24.0 * s, seed_rect.w, 10.0 * s),
            text: "Name already exists".to_string(),
            color: [1.0, 0.45, 0.45, 1.0],
            cell_px: s,
            align: ShellTextAlign::Left,
        });
    }

    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::CreateCreate,
        ShellButtonId::CreateCreate,
        "Create",
        can_create,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::CreateCancel,
        ShellButtonId::CreateCancel,
        "Cancel",
        true,
        cursor,
    );
    layout
}

fn pause_layout(screen: (u32, u32), cursor: (f32, f32)) -> ShellLayout {
    pause_skin_layout(screen, cursor, required_shell_def(ShellKind::Pause))
}

fn pause_skin_layout(screen: (u32, u32), cursor: (f32, f32), def: &ShellDef) -> ShellLayout {
    let s = gui_scale(screen);
    let mut layout = skin_layout(ShellKind::Pause);
    layout.snapshot.quads.push(ShellQuad {
        rect: rect(0.0, 0.0, screen.0 as f32, screen.1 as f32),
        color: [0.0, 0.0, 0.0, 0.56],
    });
    let panel = def.panel_rect(screen);
    layout.snapshot.texts.push(text(
        rect(panel.x, panel.y + 18.0 * s, panel.w, 12.0 * s),
        "Paused",
        1.6 * s,
        ShellTextAlign::Center,
    ));
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::PauseResume,
        ShellButtonId::PauseResume,
        "Resume",
        true,
        cursor,
    );
    push_skin_button(
        &mut layout,
        def,
        screen,
        ShellRole::PauseSaveQuit,
        ShellButtonId::PauseSaveQuit,
        "Save & Quit",
        true,
        cursor,
    );
    layout
}

fn skin_layout(kind: ShellKind) -> ShellLayout {
    ShellLayout {
        snapshot: ShellUiSnapshot {
            active: true,
            skin: Some(kind),
            ..ShellUiSnapshot::default()
        },
        buttons: Vec::new(),
        inputs: Vec::new(),
        rows: Vec::new(),
        scroll_tracks: Vec::new(),
    }
}

fn required_shell_def(kind: ShellKind) -> &'static ShellDef {
    shell_def(kind).unwrap_or_else(|| {
        panic!(
            "missing required baked shell GUI {:?}; bake it to assets/textures/gui/shell/baked",
            kind
        )
    })
}

fn push_button(
    layout: &mut ShellLayout,
    id: ShellButtonId,
    rect: SlotRect,
    label: &str,
    enabled: bool,
    cursor: (f32, f32),
) {
    layout.snapshot.buttons.push(ShellButton {
        rect,
        label: label.to_string(),
        enabled,
        hovered: enabled && rect.contains(cursor.0, cursor.1),
    });
    layout.buttons.push((id, rect, enabled));
}

#[allow(clippy::too_many_arguments)]
fn push_skin_button(
    layout: &mut ShellLayout,
    def: &ShellDef,
    screen: (u32, u32),
    role: ShellRole,
    id: ShellButtonId,
    label: &str,
    enabled: bool,
    cursor: (f32, f32),
) {
    if let Some(rect) = def.role_rect(role, screen) {
        push_button(layout, id, rect, label, enabled, cursor);
    }
}

fn push_input(
    layout: &mut ShellLayout,
    field: CreateField,
    rect: SlotRect,
    value: &TextInput,
    placeholder: &str,
    active: bool,
    scale: f32,
    now: f64,
) {
    let visible_chars = shell_input_visible_chars(rect, scale);
    let rendered = value.render(visible_chars, active, now);
    layout.snapshot.inputs.push(ShellInput {
        rect,
        text: rendered.text,
        placeholder: placeholder.to_string(),
        active,
        cursor: rendered.cursor,
        selection: rendered.selection,
        show_cursor: rendered.show_cursor,
    });
    layout.inputs.push(ShellInputHit {
        field,
        rect,
        scale,
        visible_chars,
    });
}

fn text(rect: SlotRect, label: &str, cell_px: f32, align: ShellTextAlign) -> ShellText {
    ShellText {
        rect,
        text: label.to_string(),
        color: [1.0, 1.0, 1.0, 1.0],
        cell_px,
        align,
    }
}

fn max_world_scroll(total: usize, visible: usize) -> usize {
    total.saturating_sub(visible.max(1))
}

fn scrollbar_thumb_rect(
    track: SlotRect,
    total: usize,
    visible: usize,
    scroll: usize,
) -> Option<SlotRect> {
    let max_scroll = max_world_scroll(total, visible);
    if max_scroll == 0 {
        return None;
    }
    let shown = visible.min(total).max(1) as f32;
    let total = total.max(1) as f32;
    let min_h = (track.w * 1.6).min(track.h);
    let thumb_h = (track.h * shown / total).clamp(min_h, track.h);
    let travel = (track.h - thumb_h).max(0.0);
    let t = if max_scroll == 0 {
        0.0
    } else {
        scroll.min(max_scroll) as f32 / max_scroll as f32
    };
    Some(rect(track.x, track.y + travel * t, track.w, thumb_h))
}

fn scroll_from_track_click(cursor_y: f32, track: SlotRect, total: usize, visible: usize) -> usize {
    let max_scroll = max_world_scroll(total, visible);
    if max_scroll == 0 {
        return 0;
    }
    let Some(thumb) = scrollbar_thumb_rect(track, total, visible, 0) else {
        return 0;
    };
    let travel = (track.h - thumb.h).max(1.0);
    let y = (cursor_y - track.y - thumb.h * 0.5).clamp(0.0, travel);
    (y / travel * max_scroll as f32).round() as usize
}

fn rows_bounds(rows: &[SlotRect]) -> Option<SlotRect> {
    let first = rows.first().copied()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.w;
    let mut max_y = first.y + first.h;
    for row in rows.iter().skip(1) {
        min_x = min_x.min(row.x);
        min_y = min_y.min(row.y);
        max_x = max_x.max(row.x + row.w);
        max_y = max_y.max(row.y + row.h);
    }
    Some(rect(min_x, min_y, max_x - min_x, max_y - min_y))
}

fn create_input_rects(
    screen: (u32, u32),
    panel: SlotRect,
    scale: f32,
    def: &ShellDef,
) -> (SlotRect, SlotRect) {
    let name_rect = def
        .role_rect(ShellRole::CreateNameInput, screen)
        .unwrap_or(rect(
            panel.x + 16.0 * scale,
            panel.y + 50.0 * scale,
            panel.w - 32.0 * scale,
            20.0 * scale,
        ));
    let seed_rect = def
        .role_rect(ShellRole::CreateSeedInput, screen)
        .unwrap_or(rect(
            panel.x + 16.0 * scale,
            panel.y + 88.0 * scale,
            panel.w - 32.0 * scale,
            20.0 * scale,
        ));
    (name_rect, seed_rect)
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> SlotRect {
    SlotRect { x, y, w, h }
}
