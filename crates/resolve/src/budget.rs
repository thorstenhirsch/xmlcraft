//! What loading one grammar may use up: a schema that imports a schema that imports a schema must end, and a hostile one must not be
//! able to make the app read the web.

/// The most resources one load may read.
pub const MAX_RESOURCES: usize = 256;
/// The most bytes, all resources together, one load may read.
pub const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// The most bytes one downloaded resource may have.
pub const MAX_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

/// A budget for one load, spent as resources are read.
#[derive(Clone, Debug)]
pub struct Budget {
    resources: usize,
    bytes: usize,
    max_resources: usize,
    max_bytes: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self::with_limits(MAX_RESOURCES, MAX_TOTAL_BYTES)
    }
}

impl Budget {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(max_resources: usize, max_bytes: usize) -> Self {
        Self { resources: 0, bytes: 0, max_resources, max_bytes }
    }

    /// Spend one resource of `bytes` bytes; the reason when there is not enough left (nothing is spent then).
    pub fn charge(&mut self, bytes: usize) -> Result<(), String> {
        if self.resources >= self.max_resources {
            return Err(format!("this load has read {} resources already, which is all it may read", self.max_resources));
        }
        let total = self.bytes.checked_add(bytes).filter(|total| *total <= self.max_bytes);
        let Some(total) = total else {
            return Err(format!("reading this resource would take this load past the {} bytes it may read in all", self.max_bytes));
        };
        self.resources += 1;
        self.bytes = total;
        Ok(())
    }

    /// How many resources have been read.
    pub fn resources(&self) -> usize {
        self.resources
    }

    /// How many bytes have been read.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}
