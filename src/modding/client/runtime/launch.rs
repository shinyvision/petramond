//! Client mods with no session behind them: a pack started from its
//! title-screen launch entry runs on the SHELL, and may open a presentation
//! there, which the app presents as a WORLD-LESS client built from the
//! pack's files. The launched instance is one instance throughout: it moves
//! into the presentation's runtime as it is (its state, its open UI, its
//! desks) and back out when the presentation closes.

use std::collections::BTreeSet;

use mod_api::ClientContext;

use super::super::ClientBuckets;
use super::{client_storage_dir, instantiate, load_mods, pack_storage_dir, ClientModRuntime};

/// The session key of a presentation's WORLD buckets. No session owns them
/// and nothing is ever written there (world writes are refused while a
/// presentation presents), so every read answers "absent".
const PRESENTATION_SESSION_KEY: &str = "presentation";

impl ClientModRuntime {
    /// Start pack `pack_id`'s `client_wasm` on the shell: one instance, no
    /// world bucket, its own desks.
    pub fn launch(pack_id: &str) -> Result<Self, String> {
        let path = petramond_world::assets::packs()
            .iter()
            .find(|p| p.id.as_deref() == Some(pack_id))
            .and_then(|p| p.client_wasm.clone())
            .ok_or_else(|| format!("no installed pack '{pack_id}' ships a client_wasm"))?;
        Self::launch_at(pack_id, &path)
    }

    /// [`launch`](Self::launch) of a module that is no installed pack's — a
    /// test's own guest.
    #[cfg(any(test, feature = "test-support"))]
    pub fn launch_module_for_test(pack_id: &str, module: &std::path::Path) -> Result<Self, String> {
        Self::launch_at(pack_id, module)
    }

    fn launch_at(pack_id: &str, path: &std::path::Path) -> Result<Self, String> {
        let media = super::super::media::MediaDesk::default();
        let buckets = ClientBuckets {
            world: None,
            pack: pack_storage_dir(pack_id),
        };
        let presented = super::super::presented::PresentedDesk::new(ClientContext::Shell);
        let mut launched = instantiate(pack_id, path, 0, buckets, &media, &presented)
            .ok_or_else(|| format!("client mod '{pack_id}' failed to start"))?;
        launched.launched = true;
        Ok(Self::assemble(vec![launched], media, presented))
    }

    /// The pack this runtime's shell instance was launched from, while that
    /// instance is alive.
    pub fn launched(&self) -> Option<&str> {
        self.mods
            .iter()
            .find(|m| m.launched && !m.instance.disabled())
            .map(|m| m.id.as_str())
    }

    /// The client mods of a presentation of a world with `seed` its owner
    /// opened with mods `enabled`: every installed client mod among them
    /// (they bake the presented world's custom shapes), beside this shell's
    /// launched instance carried over as it is. The desks are this
    /// runtime's, so the presentation the launched mod opened is the one it
    /// goes on driving. A listed id no installed pack ships is skipped.
    pub fn host_presentation(mut self, seed: u32, enabled: &BTreeSet<String>) -> Self {
        // Before any mod's init runs: from its first call, what a mod sees is
        // the presented world's, never a session's to keep.
        let owner = self.launched().unwrap_or_default().to_owned();
        self.presented.lock().context = ClientContext::Presentation { owner };
        let mut launched: Vec<_> = self.mods.drain(..).filter(|m| m.launched).collect();
        let carried: BTreeSet<String> = launched.iter().map(|m| m.id.clone()).collect();
        for loaded in &mut launched {
            loaded.instance.set_world_seed(seed);
            if let Some(data) = loaded.instance.client_data_mut() {
                data.shell = false;
                data.storage = Some(super::super::storage::ClientStorage::new(
                    client_storage_dir(PRESENTATION_SESSION_KEY, &loaded.id),
                ));
            }
        }
        let others: BTreeSet<String> = enabled.difference(&carried).cloned().collect();
        for id in &others {
            let installed = petramond_world::assets::packs()
                .iter()
                .any(|p| p.id.as_deref() == Some(id.as_str()));
            if !installed {
                log::warn!("presentation: pack '{id}' is not installed; presenting without it");
            }
        }
        let mut mods = load_mods(
            seed,
            PRESENTATION_SESSION_KEY,
            &others,
            &self.media,
            &self.presented,
        );
        mods.append(&mut launched);
        Self::assemble(mods, self.media.clone(), self.presented.clone())
    }

    /// Back to the shell when a presentation closes: the launched instance
    /// alone, with no world again. `None` = it did not survive the
    /// presentation (it trapped), so there is no shell to return to.
    pub fn into_shell(mut self) -> Option<Self> {
        let mut launched: Vec<_> = self
            .mods
            .drain(..)
            .filter(|m| m.launched && !m.instance.disabled())
            .collect();
        if launched.is_empty() {
            return None;
        }
        for loaded in &mut launched {
            loaded.instance.set_world_seed(0);
            if let Some(data) = loaded.instance.client_data_mut() {
                data.shell = true;
                data.storage = None;
            }
        }
        self.presented.lock().context = ClientContext::Shell;
        Some(Self::assemble(
            launched,
            self.media.clone(),
            self.presented.clone(),
        ))
    }
}
