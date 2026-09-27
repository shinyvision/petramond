use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use petramond::content::ListingRow;
use petramond::modding::ClientImageData;

pub(in crate::app) fn pack_icon_name(key: &str) -> String {
    format!("pack_icon:{key}")
}

pub(in crate::app) fn site_icon_name(mod_id: &str) -> String {
    format!("content_icon:{mod_id}")
}

fn image(key: String, rgba: image::RgbaImage) -> ClientImageData {
    ClientImageData {
        key,
        width: rgba.width() as u16,
        height: rgba.height() as u16,
        rgba: Arc::from(rgba.into_raw().into_boxed_slice()),
        revision: 1,
        recent_blits: Vec::new(),
    }
}

pub(in crate::app) fn pack_icons() -> &'static [ClientImageData] {
    static ICONS: Mutex<Option<(u64, &'static [ClientImageData])>> = Mutex::new(None);
    let serial = petramond_world::content::current().serial();
    let mut icons = ICONS.lock().unwrap();
    if let Some((loaded, values)) = *icons {
        if loaded == serial {
            return values;
        }
    }
    let values: Vec<ClientImageData> = {
        let packs = petramond_world::assets::packs().iter().map(|p| {
            (
                p.id.clone().unwrap_or_else(|| dir_name(&p.dir)),
                p.icon.clone(),
            )
        });
        let refused = petramond_world::assets::refused().iter().map(|r| {
            let key = r
                .header
                .as_ref()
                .and_then(|h| h.id.clone())
                .unwrap_or_else(|| dir_name(&r.dir));
            (key, r.header.as_ref().and_then(|h| h.icon.clone()))
        });
        packs
            .chain(refused)
            .filter_map(|(key, path)| {
                let bytes = std::fs::read(path?).ok()?;
                match petramond::content::icon::normalize(&bytes) {
                    Ok(rgba) => Some(image(pack_icon_name(&key), rgba)),
                    Err(why) => {
                        log::info!("pack '{key}': {why}");
                        None
                    }
                }
            })
            .collect()
    };
    let values: &'static [ClientImageData] = Box::leak(values.into_boxed_slice());
    *icons = Some((serial, values));
    values
}

pub(in crate::app) fn dir_name(dir: &std::path::Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

struct Want {
    mod_id: String,
    sha256: String,
    path: String,
}

pub(in crate::app) struct Icons {
    pub(in crate::app) site: Vec<ClientImageData>,
    asked: BTreeSet<(String, String)>,
    worker: Option<Sender<Want>>,
    rx: Option<Receiver<(String, image::RgbaImage)>>,
    network: bool,
}

impl Icons {
    pub(super) fn new(network: bool) -> Self {
        Self {
            site: Vec::new(),
            asked: BTreeSet::new(),
            worker: None,
            rx: None,
            network,
        }
    }

    pub(super) fn want(&mut self, rows: &[ListingRow]) {
        if !self.network {
            return;
        }
        for row in rows {
            let Some(path) = row.icon_path.clone() else {
                continue;
            };
            if !self.asked.insert((row.mod_id.clone(), row.sha256.clone())) {
                continue;
            }
            if self.worker.is_none() {
                self.start_worker();
            }
            let want = Want {
                mod_id: row.mod_id.clone(),
                sha256: row.sha256.clone(),
                path,
            };
            if self.worker.as_ref().is_some_and(|w| w.send(want).is_err()) {
                self.worker = None;
            }
        }
    }

    fn start_worker(&mut self) {
        let (want_tx, want_rx) = mpsc::channel::<Want>();
        let (done_tx, done_rx) = mpsc::channel();
        let dir: PathBuf = petramond::content::dir().join("icons");
        let spawned = std::thread::Builder::new()
            .name("petramond-content-icons".to_owned())
            .spawn(move || {
                for want in want_rx.iter() {
                    if let Some(rgba) = load(&dir, &want) {
                        if done_tx.send((want.mod_id, rgba)).is_err() {
                            return;
                        }
                    }
                }
            });
        if spawned.is_ok() {
            self.worker = Some(want_tx);
            self.rx = Some(done_rx);
        }
    }

    pub(super) fn poll(&mut self) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        while let Ok((mod_id, rgba)) = rx.try_recv() {
            let name = site_icon_name(&mod_id);
            let revision = self
                .site
                .iter()
                .find(|i| i.key == name)
                .map_or(1, |i| i.revision + 1);
            self.site.retain(|i| i.key != name);
            let mut landed = image(name, rgba);
            landed.revision = revision;
            self.site.push(landed);
        }
    }

    pub(in crate::app) fn has_site(&self, mod_id: &str) -> bool {
        let name = site_icon_name(mod_id);
        self.site.iter().any(|i| i.key == name)
    }
}

fn load(dir: &std::path::Path, want: &Want) -> Option<image::RgbaImage> {
    let cached = petramond::content::icon::cache_path(dir, &want.mod_id, &want.sha256);
    if let Ok(bytes) = std::fs::read(&cached) {
        if let Ok(rgba) = petramond::content::icon::normalize(&bytes) {
            return Some(rgba);
        }
    }
    let bytes = petramond::content::api::icon(&want.path)
        .map_err(|e| log::info!("icon for '{}': {}", want.mod_id, e.message()))
        .ok()?;
    petramond::content::icon::cache(dir, &want.mod_id, &want.sha256, &bytes)
        .map_err(|why| log::info!("icon for '{}': {why}", want.mod_id))
        .ok()
}
