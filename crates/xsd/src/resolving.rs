//! A [`SchemaLoader`] that asks the resolver: the schema documents a schema refers to are found among the documents that are open, in
//! the user's catalogs, in the bundle and at their locations, as the policy of the resolver says.

use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_text::encoding::{decode, sniff};

use crate::loader::{LoadRequest, Loaded, SchemaLoader};

/// Loads the documents of a schema through a [`Resolver`]. One loader is one load: it has a budget of its own (256 resources and
/// 64 MiB), and keeps what was looked at for each reference.
pub struct ResolvingLoader<'r> {
    resolver: &'r Resolver,
    budget: Budget,
    /// For each reference asked: what it was and where it was found, or why it was not.
    pub log: Vec<String>,
    /// The documents that were read, as (location, text): to say where in them a problem is.
    pub read: Vec<(String, String)>,
}

impl<'r> ResolvingLoader<'r> {
    pub fn new(resolver: &'r Resolver) -> Self {
        Self { resolver, budget: Budget::default(), log: Vec::new(), read: Vec::new() }
    }

    fn ask(&mut self, reference: &str, base: &str) -> Result<Loaded, String> {
        match self.resolver.resolve(&Request::uri(reference, base), &mut self.budget) {
            Ok(found) => {
                let decoded = decode(&found.bytes, &sniff(&found.bytes));
                if decoded.lossy {
                    return Err(format!("{}: it has bytes that are not valid in its encoding", found.location));
                }
                self.log.push(format!("{reference}: found at {}", found.location));
                self.read.push((found.location.clone(), decoded.text.clone()));
                Ok(Loaded { text: decoded.text, location: found.location })
            }
            Err(failure) => {
                self.log.push(format!("{reference}: {failure}"));
                Err(failure.message)
            }
        }
    }
}

impl SchemaLoader for ResolvingLoader<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        // The location the document gives, and then the namespace (the catalogs may know a schema by its namespace).
        let mut why = Vec::new();
        for reference in [request.location, request.namespace].into_iter().flatten() {
            match self.ask(reference, request.base) {
                Ok(loaded) => return Ok(loaded),
                Err(problem) => why.push(problem),
            }
        }
        if why.is_empty() { Err("it names neither a location nor a namespace".to_owned()) } else { Err(why.join("; ")) }
    }
}
