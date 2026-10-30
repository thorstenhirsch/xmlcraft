//! A [`SubsetLoader`] that asks the resolver: the external subset and the parameter entities of a DTD are found among the documents that
//! are open (a DTD being edited is validated against as it is), in the user's catalogs and in the bundle (by their public identifier:
//! `-//W3C//DTD XHTML 1.0 Strict//EN` is a file that ships with the app), and then at their locations, as the policy of the resolver says.

use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_text::encoding::{decode, sniff};

use crate::parse::{LoadRequest, Loaded, SubsetLoader};

/// The largest external text of a DTD that is used: a bigger one is refused, as a note, and not parsed.
pub const MAX_EXTERNAL_BYTES: usize = 16 * 1024 * 1024;

/// Loads the external parts of a DTD through a [`Resolver`]. One loader is one load: it has a budget of its own (256 resources and
/// 64 MiB), and keeps what it read, to say where the problems in it are.
pub struct ResolvingSubsetLoader<'r> {
    resolver: &'r Resolver,
    budget: Budget,
    read: Vec<(String, String)>,
}

impl<'r> ResolvingSubsetLoader<'r> {
    pub fn new(resolver: &'r Resolver) -> Self {
        Self { resolver, budget: Budget::default(), read: Vec::new() }
    }

    /// The texts that were read, as (where from, text).
    pub fn texts(&self) -> &[(String, String)] {
        &self.read
    }
}

impl SubsetLoader for ResolvingSubsetLoader<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let system = Some(request.system).filter(|s| !s.is_empty());
        let found = self
            .resolver
            .resolve(&Request::external_id(request.public, system, request.base), &mut self.budget)
            .map_err(|failure| failure.message)?;
        if found.bytes.len() > MAX_EXTERNAL_BYTES {
            return Err(format!("{}: over {MAX_EXTERNAL_BYTES} bytes", found.location));
        }
        let decoded = decode(&found.bytes, &sniff(&found.bytes));
        if decoded.lossy {
            return Err(format!("{}: it has bytes that are not valid in its encoding", found.location));
        }
        self.read.push((found.location.clone(), decoded.text.clone()));
        Ok(Loaded { text: decoded.text, location: found.location })
    }
}
