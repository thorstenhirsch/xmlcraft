//! How a reference was resolved: each place that was looked in, and what it said. For the user who asks why a schema was or was not
//! found, and for the tests.

use std::fmt;

/// A place the resolver looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The documents that are open.
    Buffer,
    /// The catalogs the user named.
    UserCatalog,
    /// The catalogs that ship with the app.
    BundledCatalog,
    /// The schemas that ship with the app.
    Bundle,
    /// A file.
    File,
    /// The cache of downloaded resources.
    Cache,
    /// The network.
    Network,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stage::Buffer => "open document",
            Stage::UserCatalog => "user catalog",
            Stage::BundledCatalog => "bundled catalog",
            Stage::Bundle => "bundled schema",
            Stage::File => "file",
            Stage::Cache => "cache",
            Stage::Network => "network",
        })
    }
}

/// What a place said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It has it (the text of `Step::what` says what: a location, or what a catalog maps to).
    Found,
    /// It has nothing about this.
    Missed,
    /// It was not asked, and why (offline, downloading is off).
    Skipped(String),
    /// It was asked and could not answer.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub stage: Stage,
    /// What was asked or found: an identifier, a location.
    pub what: String,
    pub outcome: Outcome,
}

/// The places looked in, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trail {
    pub steps: Vec<Step>,
}

/// The most steps a trail keeps (a hostile chain of catalogs cannot make it grow without end).
const MAX_STEPS: usize = 200;

impl Trail {
    pub fn push(&mut self, stage: Stage, what: impl Into<String>, outcome: Outcome) {
        if self.steps.len() < MAX_STEPS {
            self.steps.push(Step { stage, what: what.into(), outcome });
        }
    }

    /// The steps of one stage.
    pub fn of(&self, stage: Stage) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(move |s| s.stage == stage)
    }
}

impl fmt::Display for Trail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, step) in self.steps.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            match &step.outcome {
                Outcome::Found => write!(f, "{}: {}", step.stage, step.what)?,
                Outcome::Missed => write!(f, "{}: nothing for {}", step.stage, step.what)?,
                Outcome::Skipped(why) => write!(f, "{}: {} not asked ({why})", step.stage, step.what)?,
                Outcome::Failed(why) => write!(f, "{}: {} failed ({why})", step.stage, step.what)?,
            }
        }
        Ok(())
    }
}
