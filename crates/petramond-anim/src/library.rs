use rustc_hash::FxHashMap;

use super::expr::intern;
use petramond_world::bbmodel::{bedrock, Animation, Model};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClipId(pub(crate) u32);

impl ClipId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn from_index(index: usize) -> Self {
        ClipId(index as u32)
    }
}

#[derive(Default)]
pub struct ClipLibrary {
    clips: Vec<Animation>,
    names: Vec<String>,
    interned: Vec<f32>,
    ids: FxHashMap<String, ClipId>,
}

impl ClipLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_model(model: &Model, namespace: &str) -> Self {
        let mut names: Vec<&String> = model.animations.keys().collect();
        names.sort();
        let mut lib = Self::new();
        for name in names {
            lib.insert(
                &format!("{namespace}:{name}"),
                model.animations[name].clone(),
            );
        }
        lib
    }

    pub fn add_bedrock(
        &mut self,
        text: &str,
        rig: &Model,
        namespace: &str,
    ) -> Result<usize, String> {
        let clips = bedrock::parse_library(text, |name| rig.bone_named(name))?;
        let count = clips.len();
        for (name, clip) in clips {
            self.insert(&format!("{namespace}:{name}"), clip);
        }
        Ok(count)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn insert(&mut self, name: &str, clip: Animation) -> ClipId {
        if let Some(&id) = self.ids.get(name) {
            self.clips[id.index()] = clip;
            return id;
        }
        let id = ClipId(self.clips.len() as u32);
        self.clips.push(clip);
        self.names.push(name.to_string());
        self.interned.push(intern(name));
        self.ids.insert(name.to_string(), id);
        id
    }

    pub fn id(&self, name: &str) -> Option<ClipId> {
        self.ids.get(name).copied()
    }

    pub fn get(&self, id: ClipId) -> &Animation {
        &self.clips[id.index()]
    }

    pub fn name(&self, id: ClipId) -> &str {
        &self.names[id.index()]
    }

    pub fn interned(&self, id: ClipId) -> f32 {
        self.interned[id.index()]
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ClipId, &str, &Animation)> {
        self.clips
            .iter()
            .zip(&self.names)
            .enumerate()
            .map(|(i, (clip, name))| (ClipId(i as u32), name.as_str(), clip))
    }
}
