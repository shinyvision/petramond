use std::sync::Mutex;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct GuiKind(u8);

#[allow(non_upper_case_globals)]
impl GuiKind {
    pub const Chest: GuiKind = GuiKind(0);
    pub const Inventory: GuiKind = GuiKind(1);
    pub const CraftingTable: GuiKind = GuiKind(2);
    pub const Furnace: GuiKind = GuiKind(3);
    pub const Hotbar: GuiKind = GuiKind(4);
    pub const FurnitureWorkbench: GuiKind = GuiKind(5);
    pub const Title: GuiKind = GuiKind(6);
    pub const WorldSelect: GuiKind = GuiKind(7);
    pub const WorldSettings: GuiKind = GuiKind(8);
    pub const CreateWorld: GuiKind = GuiKind(9);
    pub const DeleteWorld: GuiKind = GuiKind(10);
    pub const Pause: GuiKind = GuiKind(11);
    pub const Demo: GuiKind = GuiKind(12);
    pub const Sleep: GuiKind = GuiKind(13);
    pub const Death: GuiKind = GuiKind(14);
    pub const ConnectServer: GuiKind = GuiKind(15);
    pub const ModsMissing: GuiKind = GuiKind(16);
    pub const ConnectionLost: GuiKind = GuiKind(17);
    pub const Options: GuiKind = GuiKind(18);
    pub const OptionsSound: GuiKind = GuiKind(19);
    pub const OptionsControls: GuiKind = GuiKind(20);
    pub const OptionsGraphics: GuiKind = GuiKind(21);
    pub const Creative: GuiKind = GuiKind(22);
    pub const Schematics: GuiKind = GuiKind(23);
    pub const ChiselingStation: GuiKind = GuiKind(24);
    pub const Account: GuiKind = GuiKind(25);
    pub const AccountSignIn: GuiKind = GuiKind(26);
    pub const Content: GuiKind = GuiKind(27);
    pub const Other: GuiKind = GuiKind(u8::MAX);

    #[inline]
    pub fn is_registered(self) -> bool {
        self.0 as usize >= ENGINE_GUI_KIND_NAMES.len() && self != GuiKind::Other
    }
}

const ENGINE_GUI_KIND_NAMES: [&str; 28] = [
    "petramond:chest",
    "petramond:inventory",
    "petramond:crafting_table",
    "petramond:furnace",
    "petramond:hotbar",
    "petramond:furniture_workbench",
    "petramond:title",
    "petramond:world_select",
    "petramond:world_settings",
    "petramond:create_world",
    "petramond:delete_world",
    "petramond:pause",
    "petramond:demo",
    "petramond:sleep",
    "petramond:death",
    "petramond:connect_server",
    "petramond:mods_missing",
    "petramond:connection_lost",
    "petramond:options",
    "petramond:options_sound",
    "petramond:options_controls",
    "petramond:options_graphics",
    "petramond:creative",
    "petramond:schematics",
    "petramond:chiseling_station",
    "petramond:account",
    "petramond:account_sign_in",
    "petramond:content",
];

const MAX_KINDS: usize = 250;

static INTERNED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

static REGISTERED_KINDS: crate::content::Slot<Mutex<Vec<&'static str>>> =
    crate::content::Slot::new("gui kinds", &[], |_| Ok(Mutex::new(Vec::new())));

pub fn intern_str(s: &str) -> &'static str {
    let mut interned = INTERNED.lock().unwrap();
    if let Some(hit) = interned.iter().find(|i| **i == s) {
        return hit;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    interned.push(leaked);
    leaked
}

pub fn intern_kind(key: &str) -> Option<GuiKind> {
    if let Some(i) = ENGINE_GUI_KIND_NAMES.iter().position(|n| *n == key) {
        return Some(GuiKind(i as u8));
    }
    if crate::registry::namespace(key) == Some(crate::registry::ENGINE_NAMESPACE) {
        return None;
    }
    if !crate::registry::is_namespaced(key) {
        return None;
    }
    let mut kinds = REGISTERED_KINDS.current().lock().unwrap();
    if let Some(i) = kinds.iter().position(|n| *n == key) {
        return Some(GuiKind((ENGINE_GUI_KIND_NAMES.len() + i) as u8));
    }
    if ENGINE_GUI_KIND_NAMES.len() + kinds.len() >= MAX_KINDS {
        log::error!("gui kind registry full; cannot register '{key}'");
        return None;
    }
    kinds.push(intern_str(key));
    Some(GuiKind(
        (ENGINE_GUI_KIND_NAMES.len() + kinds.len() - 1) as u8,
    ))
}

pub fn resolve_kind(key: &str) -> Option<GuiKind> {
    if let Some(i) = ENGINE_GUI_KIND_NAMES.iter().position(|n| *n == key) {
        return Some(GuiKind(i as u8));
    }
    let kinds = REGISTERED_KINDS.current().lock().unwrap();
    kinds
        .iter()
        .position(|n| *n == key)
        .map(|i| GuiKind((ENGINE_GUI_KIND_NAMES.len() + i) as u8))
}

pub fn engine_kind_keys() -> &'static [&'static str] {
    &ENGINE_GUI_KIND_NAMES
}

pub fn kind_key(kind: GuiKind) -> Option<&'static str> {
    let i = kind.0 as usize;
    if let Some(name) = ENGINE_GUI_KIND_NAMES.get(i) {
        return Some(name);
    }
    let kinds = REGISTERED_KINDS.current().lock().unwrap();
    kinds.get(i - ENGINE_GUI_KIND_NAMES.len()).copied()
}

impl std::fmt::Debug for GuiKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 as usize {
            0 => write!(f, "Chest"),
            1 => write!(f, "Inventory"),
            2 => write!(f, "CraftingTable"),
            3 => write!(f, "Furnace"),
            4 => write!(f, "Hotbar"),
            5 => write!(f, "FurnitureWorkbench"),
            24 => write!(f, "ChiselingStation"),
            _ if *self == GuiKind::Other => write!(f, "Other"),
            i => match kind_key(*self) {
                Some(key) => write!(f, "GuiKind({key:?})"),
                None => write!(f, "GuiKind(#{i})"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_names_resolve_to_consts_and_mod_keys_intern_once() {
        assert_eq!(intern_kind("petramond:furnace"), Some(GuiKind::Furnace));
        assert_eq!(resolve_kind("petramond:hotbar"), Some(GuiKind::Hotbar));
        assert!(!GuiKind::Furnace.is_registered());
        assert!(!GuiKind::Other.is_registered());

        assert_eq!(intern_kind("wheel"), None);
        assert_eq!(intern_kind("petramond:wheel"), None);
        let a = intern_kind("kindtest:wheel").expect("namespaced key registers");
        let b = intern_kind("kindtest:wheel").unwrap();
        assert_eq!(a, b, "re-interning returns the same id");
        assert!(a.is_registered());
        assert_eq!(kind_key(a), Some("kindtest:wheel"));
        assert_eq!(resolve_kind("kindtest:wheel"), Some(a));
        assert_eq!(resolve_kind("kindtest:never_declared"), None);
        assert_eq!(kind_key(GuiKind::Other), None);
    }

    #[test]
    fn intern_str_deduplicates() {
        let a = intern_str("kindtest:a-string");
        let b = intern_str("kindtest:a-string");
        assert!(std::ptr::eq(a, b));
    }
}
