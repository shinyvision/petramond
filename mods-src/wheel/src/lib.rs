//! wheel — the wheel-of-fortune proof-of-concept mod, the
//! Phase 5 GUI showcase: a craftable block (`wheel:wheel_of_fortune`, pack
//! data only — block + linked item + crafting recipe) whose `open_gui`
//! interaction opens the `wheel:wheel` GUI. Its `spin` button starts a
//! tick-animated `rotimage` spin that eases out over [`SPIN_TICKS`] and lands
//! exactly on a seeded random reward segment:
//!
//! - **diamond / stick / coal** → `give_item` (inventory-full overflow drops
//!   at the player, the engine's 3b semantics);
//! - **sheep party** → 5 sheep spawned on a ring around the player, each
//!   ground-dropped by a batched block-API column scan (a blocked or unloaded
//!   ring cell degrades to the player's feet);
//! - **death** → `kill_player()` (current health through the damage funnel as
//!   `DamageSource::Mod{"wheel"}`; the player's global engine i-frames apply
//!   like they do to every other damage source).
//!
//! Everything is deterministic: the reward is one `rng_u64("reward")` roll
//! (host stream, seeded per world+mod+key) and the animation is pure tick
//! math from mod-global state — no wall clock.
//!
//! # GUI state keys (session map, cleared by the engine on open AND close)
//!
//! - `wheel:angle` — `F32`, the rotimage's angle in radians (clockwise on
//!   screen). The wheel's resting angle is re-published on every open.
//! - `wheel:result` — `Str`, the landing announcement; empty until a spin
//!   lands (and cleared again by the next spin).
//!
//! # The closed-mid-spin rule
//!
//! The spinning state (start/end tick, target angle, reward) is authoritative
//! in MOD GLOBALS, not in the session map. If the GUI closes mid-spin the
//! timer keeps running and the reward is STILL delivered when it lands — the
//! wheel was already committed (simplest consistent rule; the session map is
//! only presentation). `wheel:angle`/`wheel:result` are written only while a
//! wheel session is open (tracked via `container_opened`/`container_closed`):
//! a `GuiStateSet` to a closed session would not error — the map lives on the
//! world and clears on the next open — but writing it would be pointless
//! churn. Reopening mid-spin resumes the animation from the live state;
//! reopening after a closed landing shows the landed wheel with an empty
//! result line (the announcement belongs to the session that spun).

use core::f32::consts::TAU;

use mod_sdk::*;

const SPIN_SYSTEM: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 1;
const ON_CONTAINER_CLOSED: u32 = 2;

const KIND_KEY: &str = "wheel:wheel";
const SPIN_BUTTON: &str = "spin";
const ANGLE_KEY: &str = "wheel:angle";
const RESULT_KEY: &str = "wheel:result";

/// Spin duration: 3 s at 20 TPS, easing out to an exact landing.
const SPIN_TICKS: u64 = 60;
/// Whole extra revolutions before the landing segment (travel is always in
/// `[FULL_TURNS, FULL_TURNS + 1)` turns, so every spin looks like a real one).
const FULL_TURNS: f32 = 4.0;
/// Wheel-face segments; index k's centre sits at `k * SEG_ANGLE` clockwise
/// from the pointer. MUST match the baked `wheel_face_gui.png` order.
const SEGMENTS: u64 = 5;
const SEG_ANGLE: f32 = TAU / SEGMENTS as f32;

/// Engine sound keys (assets/sounds.json): a mechanical clunk on spin start,
/// a bright UI ding on landing.
const SPIN_SOUND: &str = "petramond:chest_open";
const LAND_SOUND: &str = "petramond:item_pickup";

const SHEEP_COUNT: u32 = 5;
const SHEEP_RING_RADIUS: f32 = 3.0;
/// Ground-scan window around the player's feet for the sheep ring drop.
const SCAN_UP: i32 = 4;
const SCAN_DOWN: i32 = 8;

/// The five wheel outcomes. Discriminant == wheel-face segment index.
#[derive(Clone, Copy)]
enum Reward {
    Diamond = 0,
    Stick = 1,
    Coal = 2,
    SheepParty = 3,
    Death = 4,
}

impl Reward {
    fn from_roll(roll: u64) -> Self {
        match roll {
            0 => Reward::Diamond,
            1 => Reward::Stick,
            2 => Reward::Coal,
            3 => Reward::SheepParty,
            _ => Reward::Death,
        }
    }

    fn announcement(self) -> &'static str {
        match self {
            Reward::Diamond => "DIAMOND!",
            Reward::Stick => "a stick.",
            Reward::Coal => "coal.",
            Reward::SheepParty => "SHEEP PARTY!",
            Reward::Death => "DEATH.",
        }
    }
}

/// One in-flight spin. Lives in mod globals so it survives the GUI closing.
struct Spin {
    start_tick: u64,
    end_tick: u64,
    start_angle: f32,
    /// Absolute target angle (radians, monotonically past `start_angle`);
    /// `target % TAU` puts the reward segment's centre under the pointer.
    target_angle: f32,
    reward: Reward,
    /// The opening block's centre (from the gui_click pos) — anchors both
    /// sounds in the world; `None` for a programmatic open.
    sound_pos: Option<[f32; 3]>,
}

#[derive(Default)]
struct Wheel {
    /// Whether a `wheel:wheel` session is open (tracked via the container
    /// events) — gates the presentation writes, never the spin timer.
    gui_open: bool,
    /// The wheel's resting angle, radians in `[0, TAU)`.
    base_angle: f32,
    spin: Option<Spin>,
}

impl Mod for Wheel {
    fn init(&mut self) {
        // After(Menu): gui_click dispatches during the Menu stage, so a spin
        // started this tick animates its first frame this same tick.
        register_tick_system(Stage::Menu, AttachSide::After, 0, SPIN_SYSTEM);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_event_handler(EventKind::ContainerClosed, 0, ON_CONTAINER_CLOSED);
        log("initialized: spin animator + session tracking");
    }

    fn handle_event(&mut self, _handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match payload {
            EventPayload::ContainerOpened {
                kind: ContainerKind::Mod { key },
                ..
            } if key.as_str() == KIND_KEY => {
                self.gui_open = true;
                // The open cleared the session map; show the resting angle
                // (an in-flight spin overwrites it this same tick).
                gui_state_set(ANGLE_KEY, GuiValue::F32(self.base_angle));
            }
            EventPayload::ContainerClosed {
                kind: ContainerKind::Mod { key },
                ..
            } if key.as_str() == KIND_KEY => {
                self.gui_open = false;
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn gui_click(&mut self, kind_key: &str, widget_id: &str, pos: Option<[i32; 3]>) {
        if kind_key != KIND_KEY || widget_id != SPIN_BUTTON {
            return;
        }
        if self.spin.is_some() {
            return; // already spinning — the button is inert until it lands
        }
        let reward = Reward::from_roll(rng_u64("reward") % SEGMENTS);
        // Land with the chosen segment's centre under the pointer: rotimage
        // angles are clockwise on screen and segment k starts k*SEG_ANGLE
        // clockwise from the top, so the final angle must be ≡ -k*SEG_ANGLE
        // (mod TAU). Approach it clockwise with FULL_TURNS whole revolutions.
        let landing = (-(reward as u32 as f32) * SEG_ANGLE).rem_euclid(TAU);
        let delta = (landing - self.base_angle).rem_euclid(TAU);
        let now = current_tick();
        let sound_pos = pos.map(|p| [p[0] as f32 + 0.5, p[1] as f32 + 0.5, p[2] as f32 + 0.5]);
        emit_sound(SPIN_SOUND, sound_pos);
        // A re-spin in the same session clears the previous announcement.
        gui_state_set(RESULT_KEY, GuiValue::Str(String::new()));
        self.spin = Some(Spin {
            start_tick: now,
            end_tick: now + SPIN_TICKS,
            start_angle: self.base_angle,
            target_angle: self.base_angle + FULL_TURNS * TAU + delta,
            reward,
            sound_pos,
        });
    }

    fn tick_system(&mut self, _system_id: u32) {
        let Some(spin) = &self.spin else {
            return;
        };
        let now = current_tick();
        if now < spin.end_tick {
            // Ease-out cubic: fast off the line, decelerating into the target.
            let p = (now - spin.start_tick) as f32 / (spin.end_tick - spin.start_tick) as f32;
            let eased = 1.0 - (1.0 - p).powi(3);
            let angle = spin.start_angle + (spin.target_angle - spin.start_angle) * eased;
            if self.gui_open {
                gui_state_set(ANGLE_KEY, GuiValue::F32(angle));
            }
            return;
        }
        // Landed: rest EXACTLY on the target segment, announce, pay out. The
        // reward applies whether or not the session is still open (see the
        // closed-mid-spin rule in the module docs).
        let spin = self.spin.take().expect("checked above");
        self.base_angle = spin.target_angle.rem_euclid(TAU);
        if self.gui_open {
            gui_state_set(ANGLE_KEY, GuiValue::F32(self.base_angle));
            gui_state_set(RESULT_KEY, GuiValue::Str(spin.reward.announcement().into()));
        }
        emit_sound(LAND_SOUND, spin.sound_pos);
        apply_reward(spin.reward);
    }
}

fn apply_reward(reward: Reward) {
    match reward {
        // Engine item keys (assets/items.json). give_item overflow drops at
        // the player; an impossible-key false would only mean a broken engine
        // catalog, so the bool is deliberately ignored.
        Reward::Diamond => {
            give_item("petramond:diamond", 1);
        }
        Reward::Stick => {
            give_item("petramond:stick", 1);
        }
        Reward::Coal => {
            give_item("petramond:coal", 1);
        }
        Reward::SheepParty => sheep_party(),
        Reward::Death => kill_player(),
    }
}

/// Spawn [`SHEEP_COUNT`] sheep on a ring around the player, each dropped to
/// the ground through the block API; a ring cell with no standable ground
/// (blocked, mid-air, unloaded) degrades to the player's own feet.
fn sheep_party() {
    let player = player_state();
    for i in 0..SHEEP_COUNT {
        let angle = TAU * i as f32 / SHEEP_COUNT as f32;
        let (sin, cos) = angle.sin_cos();
        let wx = (player.pos[0] + SHEEP_RING_RADIUS * cos).floor() as i32;
        let wz = (player.pos[2] + SHEEP_RING_RADIUS * sin).floor() as i32;
        let pos = match ground_y(wx, player.pos[1].floor() as i32, wz) {
            Some(y) => [wx as f32 + 0.5, y as f32, wz as f32 + 0.5],
            None => player.pos,
        };
        spawn_mob("petramond:sheep", pos, angle);
    }
}

/// Feet Y of the highest standable cell in column `(wx, wz)` near `py`: a
/// non-air block below and two air cells of headroom, scanned top-down (the
/// monsters mod's PoC ground rule). `None` when unloaded or no such cell.
fn ground_y(wx: i32, py: i32, wz: i32) -> Option<i32> {
    if !is_loaded([wx, py, wz]) {
        return None;
    }
    let ys: Vec<i32> = (py - SCAN_DOWN..=py + SCAN_UP + 1).collect();
    let column = get_blocks(ys.iter().map(|&y| [wx, y, wz]).collect());
    for i in (1..column.len() - 1).rev() {
        let solid_below = matches!(column[i - 1], Some(b) if b != BlockId::AIR);
        let clear = column[i] == Some(BlockId::AIR) && column[i + 1] == Some(BlockId::AIR);
        if solid_below && clear {
            return Some(ys[i]);
        }
    }
    None
}

register_mod!(Wheel);
