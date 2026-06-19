//! Biome definitions + selection from climate (6 parameters).

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Ocean,
    Beach,
    River,
    Desert,
    Plains,
    Savanna,
    Forest,
    BirchForest,
    Swamp,
    Taiga,
    SnowyTundra,
    SnowyTaiga,
    Mountains,
    SnowyPeaks,
    DeepOcean,
    Foothills,
    Wetland,
}

/// 6-parameter climate sample, each in [-1, 1].
#[derive(Copy, Clone, Debug)]
pub struct Climate {
    pub temperature: f32,
    pub humidity: f32,
    pub continentalness: f32,
    pub erosion: f32,
    pub weirdness: f32,
    pub depth: f32,
}

impl Climate {
    /// Helper convenience: temperature 0..1.
    pub fn temp01(self) -> f32 { (self.temperature * 0.5 + 0.5).clamp(0.0, 1.0) }
    pub fn humid01(self) -> f32 { (self.humidity * 0.5 + 0.5).clamp(0.0, 1.0) }
    pub fn cont01(self) -> f32 { (self.continentalness * 0.5 + 0.5).clamp(0.0, 1.0) }
    pub fn erode01(self) -> f32 { (self.erosion * 0.5 + 0.5).clamp(0.0, 1.0) }
}

/// Pick biome from climate + surface height. Ordered cascade: oceans (depth-led),
/// shore, high-altitude (foothills/mountains), then a temperature×humidity grid
/// for the lowlands. Transition bands (Beach/Foothills/Wetland) are jittered by
/// `weirdness` so they appear as ragged intermittent patches, never closed rings.
pub fn biome_at(c: Climate, surf_y: i32) -> Biome {
    use Biome::*;
    let sea = crate::chunk::SEA_LEVEL; // 64
    let t = c.temp01();
    let h = c.humid01();
    // Edge dither: weirdness is an independent fbm, so the band edges wander.
    let ey = (c.weirdness * 14.0) as i32; // ~±3 block altitude jitter

    // ---- Oceans (depth-led; floors sit well below sea level) ----
    if surf_y <= 46 + ey / 2 {
        return DeepOcean;
    }
    if surf_y <= 61 + ey / 2 {
        return Ocean;
    }

    // ---- Beach: a thin shore slab, but only on scattered stretches — gated on an
    // independent noise so it does NOT form a closed ring around every coast.
    // Where it doesn't form, the coast falls through to grass / wetland down to the
    // waterline (varied shores). Cold shores stay non-sandy.
    if surf_y <= 64 + ey && c.weirdness > -0.05 && t > 0.30 {
        return Beach;
    }

    // ---- High altitude: mountains + their foothill transition ----
    if surf_y > 100 + ey {
        return if t < 0.30 { SnowyPeaks } else { Mountains };
    }
    if surf_y > 88 + ey {
        return Foothills;
    }

    // ---- Wetland / Swamp: humid low land near the waterline ----
    if surf_y <= sea + 6 + ey && h > 0.60 {
        if h > 0.74 {
            return Swamp;
        }
        return Wetland;
    }

    // ---- Lowland temperature × humidity grid ----
    if t < 0.30 {
        return if h < 0.42 { SnowyTundra } else { SnowyTaiga };
    }
    if t > 0.70 {
        if h < 0.32 {
            return Desert;
        }
        if h < 0.55 {
            return Savanna;
        }
        return Forest; // hot + humid
    }
    if h > 0.58 {
        if t < 0.38 {
            return Taiga;
        }
        return Forest;
    }
    if h > 0.40 {
        if t < 0.38 {
            return Taiga;
        }
        return if t > 0.62 { BirchForest } else { Forest };
    }
    Plains // temperate-dry default
}

impl Biome {
    pub fn fog_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.30, 0.45, 0.85],
            Biome::DeepOcean => [0.16, 0.28, 0.62],
            Biome::Swamp => [0.44, 0.54, 0.58],
            Biome::Wetland => [0.50, 0.60, 0.62],
            Biome::River => [0.55, 0.66, 0.78],
            Biome::Desert | Biome::Beach => [0.93, 0.88, 0.70],
            Biome::Foothills | Biome::Mountains => [0.65, 0.77, 0.92],
            Biome::SnowyTundra | Biome::SnowyPeaks | Biome::SnowyTaiga => [0.85, 0.90, 0.98],
            _ => [0.62, 0.78, 0.95],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::Beach => "beach",
            Biome::River => "river",
            Biome::Desert => "desert",
            Biome::Plains => "plains",
            Biome::Savanna => "savanna",
            Biome::Forest => "forest",
            Biome::BirchForest => "birch_forest",
            Biome::Swamp => "swamp",
            Biome::Taiga => "taiga",
            Biome::SnowyTundra => "snowy_tundra",
            Biome::SnowyTaiga => "snowy_taiga",
            Biome::Mountains => "mountains",
            Biome::SnowyPeaks => "snowy_peaks",
            Biome::DeepOcean => "deep_ocean",
            Biome::Foothills => "foothills",
            Biome::Wetland => "wetland",
        }
    }

    pub fn from_id(id: u8) -> Biome {
        match id {
            1 => Biome::Beach,
            2 => Biome::River,
            3 => Biome::Desert,
            4 => Biome::Plains,
            5 => Biome::Savanna,
            6 => Biome::Forest,
            7 => Biome::BirchForest,
            8 => Biome::Swamp,
            9 => Biome::Taiga,
            10 => Biome::SnowyTundra,
            11 => Biome::SnowyTaiga,
            12 => Biome::Mountains,
            13 => Biome::SnowyPeaks,
            14 => Biome::DeepOcean,
            15 => Biome::Foothills,
            16 => Biome::Wetland,
            _ => Biome::Ocean,
        }
    }

    pub fn id(self) -> u8 { self as u8 }

    /// Grass-block top tint colour (linear sRGB 0..1) for biome. Forest/Plains are
    /// a normal saturated green; Foothills/Mountains are desaturated (R≈G); Desert
    /// is a deadish yellow, Savanna a yellow-green, Wetland dark green, Swamp darker.
    pub fn grass_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.48, 0.68, 0.40],
            Biome::Beach => [0.66, 0.72, 0.42],
            Biome::River => [0.48, 0.70, 0.42],
            Biome::Desert => [0.80, 0.72, 0.34],
            Biome::Savanna => [0.69, 0.69, 0.31],
            Biome::Plains => [0.50, 0.73, 0.34],
            Biome::Forest => [0.40, 0.66, 0.30],
            Biome::BirchForest => [0.56, 0.72, 0.40],
            Biome::Swamp => [0.30, 0.44, 0.24],
            Biome::Taiga => [0.44, 0.60, 0.40],
            Biome::SnowyTundra => [0.62, 0.72, 0.58],
            Biome::SnowyTaiga => [0.52, 0.66, 0.50],
            Biome::Mountains => [0.50, 0.62, 0.42],
            Biome::SnowyPeaks => [0.80, 0.86, 0.82],
            Biome::DeepOcean => [0.44, 0.64, 0.38],
            Biome::Foothills => [0.52, 0.64, 0.44],
            Biome::Wetland => [0.34, 0.52, 0.28],
        }
    }

    /// Foliage tint (leaves) for biome.
    pub fn foliage_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.44, 0.64, 0.36],
            Biome::Beach => [0.60, 0.68, 0.38],
            Biome::River => [0.44, 0.66, 0.38],
            Biome::Desert => [0.74, 0.66, 0.30],
            Biome::Savanna => [0.62, 0.62, 0.28],
            Biome::Plains => [0.46, 0.70, 0.30],
            Biome::Forest => [0.34, 0.60, 0.24],
            Biome::BirchForest => [0.58, 0.74, 0.40],
            Biome::Swamp => [0.26, 0.40, 0.20],
            Biome::Taiga => [0.40, 0.58, 0.36],
            Biome::SnowyTundra => [0.58, 0.70, 0.56],
            Biome::SnowyTaiga => [0.48, 0.64, 0.48],
            Biome::Mountains => [0.46, 0.58, 0.38],
            Biome::SnowyPeaks => [0.74, 0.82, 0.74],
            Biome::DeepOcean => [0.40, 0.60, 0.34],
            Biome::Foothills => [0.48, 0.60, 0.40],
            Biome::Wetland => [0.30, 0.48, 0.24],
        }
    }

    /// Water tint for biome. Ocean is a normal blue, DeepOcean a much darker blue,
    /// Swamp/Wetland a murky green-blue.
    pub fn water_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.16, 0.34, 0.74],
            Biome::Beach => [0.20, 0.40, 0.74],
            Biome::River => [0.20, 0.42, 0.66],
            Biome::Desert => [0.24, 0.44, 0.72],
            Biome::Savanna => [0.26, 0.44, 0.68],
            Biome::Plains => [0.20, 0.40, 0.74],
            Biome::Forest => [0.18, 0.36, 0.66],
            Biome::BirchForest => [0.22, 0.42, 0.64],
            Biome::Swamp => [0.24, 0.36, 0.30],
            Biome::Taiga => [0.20, 0.38, 0.56],
            Biome::SnowyTundra => [0.30, 0.46, 0.66],
            Biome::SnowyTaiga => [0.28, 0.44, 0.60],
            Biome::Mountains => [0.22, 0.42, 0.64],
            Biome::SnowyPeaks => [0.34, 0.50, 0.68],
            Biome::DeepOcean => [0.07, 0.18, 0.50],
            Biome::Foothills => [0.20, 0.40, 0.66],
            Biome::Wetland => [0.26, 0.40, 0.40],
        }
    }
}