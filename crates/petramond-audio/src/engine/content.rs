use super::{decode, sound_defs, Audio, DecodedSound};

impl Audio {
    pub fn reload_content(&mut self) {
        self.loop_sound = None;
        self.spatial.clear();
        self.oneshots.clear();
        self.gain_loops.clear();
        self.music = None;
        self.buffers = load_buffers();
    }
}

pub(super) fn load_buffers() -> Vec<Vec<DecodedSound>> {
    sound_defs()
        .iter()
        .map(|def| {
            def.variants
                .iter()
                .filter_map(|&rel| {
                    let Some((bytes, _)) = petramond_world::assets::read_bytes(rel) else {
                        log::warn!("sound {:?} clip '{rel}' not found (skipped)", def.sound);
                        return None;
                    };
                    match decode(bytes) {
                        Ok(decoded) => Some(decoded),
                        Err(error) => {
                            log::warn!(
                                "sound {:?} variant failed to decode (skipped): {error}",
                                def.sound
                            );
                            None
                        }
                    }
                })
                .collect()
        })
        .collect()
}
