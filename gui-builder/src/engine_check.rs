//! The builder's host side of the game's load-time document validation.
//!
//! Every rule comes from `petramond_ui::contract` — the same functions the
//! game's loader calls — so there is no builder copy of any table. This
//! module only supplies what those rules ask the host for: the pack the
//! document ships in (namespace ownership), the item tags every asset layer
//! registers (slot `accepts`), and image files beside the project.

use crate::assets::{self, AssetRoots};
use petramond_ui::contract::{self, EngineCatalog, EngineCheck};
use petramond_ui::{DocClass, DocIssue, Document, Theme, UiState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// GUI scales the viewport-fit rule is judged at: the smallest, and the one
/// where secondary-size text is proportionally largest.
const FIT_SCALES: [i32; 2] = [1, 3];

/// What the engine would know about a document at load: its shipping pack
/// and the item tags registered by the base game plus that pack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineContext {
    /// The id from the nearest enclosing `pack.json` (`None`: base assets or
    /// a loose project outside any pack).
    pub pack_id: Option<String>,
    item_tags: HashSet<String>,
}

impl EngineContext {
    /// The context for a project saved in `project_dir`, seen through
    /// `roots` (the project's layered roots — see
    /// [`AssetRoots::for_project`]): the enclosing pack (the first ancestor
    /// holding a `pack.json`) and the tags every layer's `items.json` rows
    /// list.
    pub fn for_project(roots: &AssetRoots, project_dir: Option<&Path>) -> EngineContext {
        let pack_id = project_dir
            .and_then(assets::enclosing_pack)
            .as_deref()
            .and_then(read_pack_id);
        let mut item_tags = HashSet::new();
        for path in roots.all("items.json") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(json) = serde_json::from_str(&text) {
                    collect_tags(&json, &mut item_tags);
                }
            }
        }
        EngineContext { pack_id, item_tags }
    }

    /// Every reason the game would refuse `doc` at load, plus the
    /// smallest-viewport fit rule for non-HUD documents, judged with the
    /// preview's sample `state`. `image_path` resolves a static image name
    /// the way the exported document will.
    pub fn validate(
        &self,
        doc: &Document,
        theme: &Theme,
        state: &UiState,
        image_path: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> Vec<DocIssue> {
        let image_size =
            |name: &str| image_path(name).and_then(|p| image::image_dimensions(p).ok());
        let mut issues = contract::validate_for_engine(
            doc,
            &EngineCheck {
                styles: Some(theme),
                pack_id: self.pack_id.as_deref(),
                catalog: self,
                image_size: &image_size,
            },
        );
        if doc.class != DocClass::Hud {
            for scale in FIT_SCALES {
                for issue in contract::viewport_overflow(doc, theme, state, scale, &|_| None) {
                    // One finding per node: the smallest scale that fails.
                    if !issues.iter().any(|i| i.path == issue.path) {
                        issues.push(issue);
                    }
                }
            }
        }
        issues
    }
}

impl EngineCatalog for EngineContext {
    /// Mirrors the engine's non-interning tag query: engine tags are listed
    /// bare on base rows and may be named `petramond:<tag>`; pack tags are
    /// namespaced and match exactly.
    fn item_tag_exists(&self, name: &str) -> bool {
        let bare = name
            .strip_prefix(contract::ENGINE_NAMESPACE)
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(name);
        self.item_tags.contains(bare) || self.item_tags.contains(name)
    }
}

fn read_pack_id(pack_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pack_root.join("pack.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("id")?.as_str().map(str::to_owned)
}

/// Every string listed under a `tags` key anywhere in an item catalog.
fn collect_tags(json: &serde_json::Value, out: &mut HashSet<String>) {
    match json {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value) {
                    ("tags", serde_json::Value::Array(tags)) => {
                        out.extend(tags.iter().filter_map(|t| t.as_str().map(str::to_owned)));
                    }
                    _ => collect_tags(value, out),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_tags(v, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gui-builder-engine-check-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_project_inside_a_pack_takes_its_id_and_tags() {
        let root = scratch("pack");
        let docs = root.join("ui/documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(
            root.join("pack.json"),
            r#"{ "id": "doctest", "name": "T" }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("items.json"),
            r#"{ "items": [ { "key": "doctest:ingot", "tags": ["doctest:metal"] } ] }"#,
        )
        .unwrap();
        let roots = AssetRoots::default().for_project(Some(&docs), &[]);
        let ctx = EngineContext::for_project(&roots, Some(&docs));
        assert_eq!(ctx.pack_id.as_deref(), Some("doctest"));
        assert!(ctx.item_tag_exists("doctest:metal"));
        assert!(!ctx.item_tag_exists("doctest:no_such_tag"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn engine_tags_answer_bare_and_namespaced() {
        let mut ctx = EngineContext::default();
        ctx.item_tags.insert("fuel".into());
        assert!(ctx.item_tag_exists("fuel"));
        assert!(ctx.item_tag_exists("petramond:fuel"));
        assert!(!ctx.item_tag_exists("othermod:fuel"));
    }

    /// The builder reports what the game refuses: a mod kind outside its
    /// pack, a role mod documents may not use, and an unknown tag — none of
    /// which the structural `Document::validate` alone sees.
    #[test]
    fn builder_validation_runs_the_engine_rules() {
        let doc = Document::from_json(
            r#"{ "format": 1, "kind": "doctest:machine", "class": "container",
                 "root": { "type": "column", "children": [
                     { "type": "slot", "role": "craft_result" },
                     { "type": "slot", "role": "container", "accepts": ["doctest:nope"] }
                 ] } }"#,
        )
        .unwrap();
        let ctx = EngineContext::default();
        let theme = Theme::placeholder();
        let issues = ctx.validate(&doc, &theme, &UiState::new(), &|_| None);
        let text: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        assert!(
            text.iter().any(|m| m.contains("ships outside any pack")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|m| m.contains("not available to mod documents")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|m| m.contains("unknown item tag")),
            "{text:?}"
        );
    }
}
