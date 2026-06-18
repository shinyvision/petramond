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

/// Pick biome from climate + surface height (depth via absolute Y).
pub fn biome_at(c: Climate, surf_y: i32) -> Biome {
    // Ocean / beach by continent (low = sea).
    if c.continentalness < -0.25 {
        return Biome::Ocean;
    }
    if c.continentalness < -0.15 && surf_y <= crate::chunk::SEA_LEVEL {
        return Biome::Ocean;
    }
    if surf_y <= crate::chunk::SEA_LEVEL + 1 && c.continentalness < 0.0 {
        return Biome::Beach;
    }

    // High altitude takes precedence.
    if surf_y > 105 {
        return if c.temp01() < 0.25 { Biome::SnowyPeaks } else { Biome::Mountains };
    }
    if surf_y > 82 {
        // Mountainous transition; cold enough => snow caps.
        if c.temp01() < 0.2 { return Biome::SnowyPeaks; }
        if c.temp01() < 0.4 { return Biome::SnowyTaiga; }
        return Biome::Mountains;
    }

    // Temperature axis first (cold/hot extremes), then humidity.
    let t = c.temp01();
    let h = c.humid01();

    if t < 0.15 {
        if h < 0.4 { return Biome::SnowyTundra; }
        return Biome::SnowyTaiga;
    }
    if t > 0.85 {
        if h < 0.25 { return Biome::Desert; }
        if h < 0.5  { return Biome::Savanna; }
    }
    if h > 0.7 && t < 0.45 {
        return Biome::Swamp;
    }
    if h > 0.55 {
        if t < 0.35 { return Biome::Taiga; }
        if t < 0.6  { return Biome::Forest; }
        return Biome::Forest; // dense temperate
    }
    if h > 0.35 {
        if t < 0.45 { return Biome::Taiga; }
        if t < 0.75 { return Biome::BirchForest; }
        return Biome::Savanna;
    }
    // Default temperate dry -> plains.
    Biome::Plains
}

impl Biome {
    pub fn fog_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.30, 0.45, 0.85],
            Biome::River | Biome::Swamp => [0.45, 0.55, 0.65],
            Biome::Desert | Biome::Beach => [0.95, 0.88, 0.70],
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
            _ => Biome::Ocean,
        }
    }

    pub fn id(self) -> u8 { self as u8 }

    /// Grass-block top tint colour (linear sRGB 0..1) for biome.
    pub fn grass_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.55, 0.74, 0.45],
            Biome::Beach => [0.66, 0.73, 0.42],
            Biome::River => [0.50, 0.72, 0.45],
            Biome::Desert => [0.78, 0.72, 0.42],
            Biome::Savanna => [0.72, 0.66, 0.35],
            Biome::Plains => [0.55, 0.74, 0.36],
            Biome::Forest => [0.42, 0.66, 0.32],
            Biome::BirchForest => [0.60, 0.74, 0.42],
            Biome::Swamp => [0.38, 0.55, 0.30],
            Biome::Taiga => [0.45, 0.62, 0.40],
            Biome::SnowyTundra => [0.55, 0.70, 0.55],
            Biome::SnowyTaiga => [0.50, 0.66, 0.50],
            Biome::Mountains => [0.52, 0.69, 0.42],
            Biome::SnowyPeaks => [0.78, 0.84, 0.78],
        }
    }

    /// Foliage tint (leaves) for biome.
    pub fn foliage_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.55, 0.74, 0.40],
            Biome::Beach => [0.60, 0.72, 0.38],
            Biome::River => [0.48, 0.72, 0.38],
            Biome::Desert => [0.70, 0.62, 0.32],
            Biome::Savanna => [0.62, 0.58, 0.30],
            Biome::Plains => [0.52, 0.72, 0.32],
            Biome::Forest => [0.38, 0.62, 0.28],
            Biome::BirchForest => [0.62, 0.76, 0.40],
            Biome::Swamp => [0.34, 0.52, 0.26],
            Biome::Taiga => [0.40, 0.60, 0.36],
            Biome::SnowyTundra => [0.50, 0.68, 0.50],
            Biome::SnowyTaiga => [0.46, 0.64, 0.46],
            Biome::Mountains => [0.48, 0.67, 0.36],
            Biome::SnowyPeaks => [0.72, 0.82, 0.70],
        }
    }

    /// Water tint for biome.
    pub fn water_color(self) -> [f32; 3] {
        match self {
            Biome::Ocean => [0.20, 0.32, 0.62],
            Biome::Beach => [0.30, 0.46, 0.70],
            Biome::River => [0.25, 0.45, 0.62],
            Biome::Desert => [0.30, 0.46, 0.70],
            Biome::Savanna => [0.28, 0.44, 0.66],
            Biome::Plains => [0.24, 0.42, 0.66],
            Biome::Forest => [0.20, 0.38, 0.58],
            Biome::BirchForest => [0.24, 0.42, 0.62],
            Biome::Swamp => [0.30, 0.38, 0.32],
            Biome::Taiga => [0.22, 0.38, 0.52],
            Biome::SnowyTundra => [0.30, 0.46, 0.66],
            Biome::SnowyTaiga => [0.28, 0.44, 0.60],
            Biome::Mountains => [0.24, 0.40, 0.60],
            Biome::SnowyPeaks => [0.34, 0.50, 0.68],
        }
    }
}