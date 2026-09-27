//! Client key actions: the ABI's key names, the default-collision rule, and
//! where a registered action may fire.

use petramond_input::controls::{
    fixed_control_from_key_code, BindMods, BindableAction, Binding, BindingSet, BoundInput,
};
use petramond_input::keycode::KeyCode;

/// The ABI name of a physical key: the snake_case of its position name —
/// `KeyA` → `key_a`, `Digit1` → `digit_1`, `ArrowLeft` → `arrow_left`,
/// `Numpad0` → `numpad_0`, `F9` → `f9` (a one-letter word keeps its number).
pub fn key_name(code: KeyCode) -> String {
    let name = code.name();
    let mut out = String::with_capacity(name.len() + 4);
    let mut word_len = 0usize;
    let mut prev_digit = false;
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            word_len = 1;
            prev_digit = false;
        } else if ch.is_ascii_digit() {
            if !prev_digit && word_len > 1 {
                out.push('_');
            }
            out.push(ch);
            prev_digit = true;
        } else {
            out.push(ch);
            word_len += 1;
            prev_digit = false;
        }
    }
    out
}

/// The key behind an ABI key name, or `None` for a name no key has.
pub fn key_code_for_name(name: &str) -> Option<KeyCode> {
    KeyCode::ALL
        .iter()
        .copied()
        .find(|&code| key_name(code) == name)
}

/// The engine binding a registered DEFAULT is.
pub(super) fn default_binding(code: KeyCode, mods: mod_api::ClientKeyMods) -> Binding {
    Binding {
        mods: BindMods {
            ctrl: mods.ctrl,
            shift: mods.shift,
            alt: mods.alt,
            meta: false,
        },
        input: BoundInput::Key(code),
    }
}

/// Why a registered default may not stand, if it may not. A default the
/// player would meet in GAMEPLAY must not equal an engine default binding
/// (key and chord) or a bare fixed control — the player could no longer
/// tell who owns the key out of the box. A default that fires only over the
/// mod's own screens shadows nothing there but Escape, which always belongs
/// to the engine. Remaps are the player's own choice and are not policed.
pub(super) fn default_refusal(binding: Binding, gameplay: bool) -> Option<&'static str> {
    if binding.input == BoundInput::Key(KeyCode::Escape) {
        return Some("Escape always belongs to the engine");
    }
    if !gameplay {
        return None;
    }
    let BoundInput::Key(code) = binding.input else {
        return None;
    };
    if binding.mods.is_empty() && fixed_control_from_key_code(code).is_some() {
        return Some("it is a fixed engine control");
    }
    let defaults = BindingSet::default();
    BindableAction::ALL
        .iter()
        .any(|action| defaults.binding(*action) == binding)
        .then_some("it is an engine default binding")
}

/// Where one registered action may fire, as the app reports the frame: the
/// world takes gameplay input, and which client screen (document kind or
/// canvas key) is open.
#[derive(Copy, Clone, Debug, Default)]
pub struct KeyContext<'a> {
    pub gameplay: bool,
    pub screen: Option<&'a str>,
}

pub(super) fn fires_in(contexts: &mod_api::ClientKeyContexts, at: KeyContext<'_>) -> bool {
    (contexts.gameplay && at.gameplay)
        || at
            .screen
            .is_some_and(|open| contexts.screens.iter().any(|s| s == open))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_name_resolves_back_to_its_key() {
        for &code in KeyCode::ALL {
            assert_eq!(key_code_for_name(&key_name(code)), Some(code), "{code:?}");
        }
        assert_eq!(key_name(KeyCode::KeyM), "key_m");
        assert_eq!(key_name(KeyCode::Digit1), "digit_1");
        assert_eq!(key_name(KeyCode::F9), "f9");
        assert_eq!(key_name(KeyCode::ArrowLeft), "arrow_left");
        assert_eq!(key_name(KeyCode::Numpad0), "numpad_0");
    }

    #[test]
    fn only_a_gameplay_default_is_measured_against_the_engine() {
        let mods = |ctrl| mod_api::ClientKeyMods {
            ctrl,
            ..Default::default()
        };
        // A chord and its bare key are different bindings, so an engine chord
        // no longer poisons its bare key (Ctrl+Z used to refuse Z).
        let default_chord = BindableAction::ALL
            .iter()
            .map(|a| BindingSet::default().binding(*a))
            .find(|b| !b.mods.is_empty())
            .expect("the engine has at least one chord default");
        assert!(default_refusal(default_chord, true).is_some());
        assert!(default_refusal(default_chord, false).is_none());
        let bare = Binding {
            mods: BindMods::default(),
            ..default_chord
        };
        let bare_is_engine = BindableAction::ALL
            .iter()
            .any(|a| BindingSet::default().binding(*a) == bare);
        assert_eq!(
            default_refusal(bare, true).is_some(),
            bare_is_engine
                || matches!(bare.input, BoundInput::Key(c) if fixed_control_from_key_code(c).is_some())
        );
        let fixed = default_binding(KeyCode::KeyV, mods(false));
        assert!(default_refusal(fixed, true).is_some());
        assert!(default_refusal(default_binding(KeyCode::KeyV, mods(true)), true).is_none());
        let escape = default_binding(KeyCode::Escape, mods(false));
        assert!(default_refusal(escape, false).is_some());
    }
}
