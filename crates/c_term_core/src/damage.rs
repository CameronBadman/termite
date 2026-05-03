pub type Generation = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageRegion {
    Cells {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    Cursor {
        old: Option<(u16, u16)>,
        new: (u16, u16),
    },
    Viewport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageBatch {
    pub generation: Generation,
    pub regions: Vec<DamageRegion>,
}

impl DamageBatch {
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct DamageTracker {
    regions: Vec<DamageRegion>,
}

impl DamageTracker {
    pub fn mark(&mut self, region: DamageRegion) {
        self.regions.push(region);
    }

    pub fn drain(&mut self, generation: Generation) -> DamageBatch {
        DamageBatch {
            generation,
            regions: std::mem::take(&mut self.regions),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}
