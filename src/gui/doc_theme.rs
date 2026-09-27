use petramond_ui::{Theme, ThemeLayer};
use std::sync::{Arc, Mutex};

const THEME_JSON: &str = "ui/theme/theme.json";

static THEME: Mutex<Option<Arc<Theme>>> = Mutex::new(None);

pub fn theme() -> Arc<Theme> {
    THEME.lock().unwrap().get_or_insert_with(load).clone()
}

pub fn reload() {
    *THEME.lock().unwrap() = None;
}

pub fn ui_font() -> Arc<petramond_ui::text::Font> {
    theme().ui_font().clone()
}

fn load() -> Arc<Theme> {
    let layers = petramond_world::assets::read_layers(THEME_JSON);
    if layers.is_empty() {
        return Arc::new(Theme::placeholder());
    }
    let readers: Vec<_> = layers
        .iter()
        .map(|(_, path)| {
            let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            move |rel: &str| std::fs::read(dir.join(rel)).ok()
        })
        .collect();
    let stack: Vec<ThemeLayer<'_>> = layers
        .iter()
        .zip(&readers)
        .map(|((json, _), read)| ThemeLayer { json, read })
        .collect();
    let describe = |n: usize| {
        layers[..n]
            .iter()
            .map(|(_, path)| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match Theme::load_stack(&stack) {
        Ok(theme) => return Arc::new(theme),
        Err(e) => eprintln!(
            "gui: theme stack [{}] failed to load — {e}",
            describe(stack.len())
        ),
    }
    if stack.len() > 1 {
        match Theme::load_stack(&stack[..1]) {
            Ok(theme) => {
                eprintln!("gui: using the base theme without pack layers");
                return Arc::new(theme);
            }
            Err(e) => eprintln!("gui: base theme {} failed to load — {e}", describe(1)),
        }
    }
    eprintln!("gui: using the placeholder theme");
    Arc::new(Theme::placeholder())
}
