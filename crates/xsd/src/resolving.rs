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

    fn ask(&mut self, reference: &str, base: &str, lookup_only: bool) -> Result<Loaded, String> {
        let request = Request::uri(reference, base);
        match self.resolver.resolve(&if lookup_only { request.lookup_only() } else { request }, &mut self.budget) {
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
    fn identity(&self, location: &str) -> String {
        self.resolver.identity(location)
    }

    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        // The location the document gives, and then the namespace (the catalogs may know a schema by its namespace; a namespace is a name,
        // not an address, so it is looked for there and is never read as a file or downloaded). A location that is empty names nothing.
        let mut why = Vec::new();
        for (reference, lookup_only) in [(request.location, false), (request.namespace, true)] {
            let Some(reference) = reference.filter(|r| !r.is_empty()) else { continue };
            match self.ask(reference, request.base, lookup_only) {
                Ok(loaded) => return Ok(loaded),
                Err(problem) => why.push(problem),
            }
        }
        if why.is_empty() { Err("it names neither a location nor a namespace".to_owned()) } else { Err(why.join("; ")) }
    }
}
