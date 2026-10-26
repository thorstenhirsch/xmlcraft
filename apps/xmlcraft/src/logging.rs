//! The desktop app's logger: `log::` records go to standard error.
//!
//! Levels: `info` for XmlCraft's own crates, `warn` for everything else (wgpu and naga are chatty). `RUST_LOG` replaces that with
//! env_logger-style directives: `debug`, `warn,xmlcraft_syntax=trace`, `wgpu_core=info`. A directive ending in `*` matches every
//! target that starts with it (`xmlcraft*=debug`). Never log a control-channel token or any document content.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

/// The built-in filter when `RUST_LOG` is unset or empty.
pub const DEFAULT_FILTER: &str = "warn,xmlcraft*=info";

/// Per-target level filter parsed from env_logger-style directives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    default: LevelFilter,
    directives: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// Parse `spec`; unknown levels and empty parts are skipped. Without a bare level, targets no directive names log errors only.
    pub fn parse(spec: &str) -> Filter {
        let mut f = Filter { default: LevelFilter::Error, directives: Vec::new() };
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((name, level)) => {
                    let name = name.trim();
                    if let (false, Some(level)) = (name.is_empty(), parse_level(level)) {
                        f.directives.push((name.to_owned(), level));
                    }
                }
                None => match parse_level(part) {
                    Some(level) => f.default = level,
                    // A bare target name: everything from it (env_logger does the same).
                    None => f.directives.push((part.to_owned(), LevelFilter::Trace)),
                },
            }
        }
        f
    }

    /// The level that applies to `target` (the most specific matching directive wins).
    pub fn level_for(&self, target: &str) -> LevelFilter {
        self.directives.iter().filter(|(name, _)| matches(name, target)).max_by_key(|(name, _)| name.len()).map_or(self.default, |(_, level)| *level)
    }

    /// The most verbose level any target can reach (for `log::set_max_level`).
    pub fn max(&self) -> LevelFilter {
        self.directives.iter().map(|(_, level)| *level).fold(self.default, Ord::max)
    }
}

fn parse_level(s: &str) -> Option<LevelFilter> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// Whether `directive` names `target`: equal, a module path prefix (`a::b` names `a::b::c`), or a `*` prefix.
fn matches(directive: &str, target: &str) -> bool {
    match directive.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => target == directive || target.strip_prefix(directive).is_some_and(|rest| rest.starts_with("::")),
    }
}

struct Logger {
    filter: Filter,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level_for(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Writing never panics: a closed standard error just loses the record.
        let _ = writeln!(std::io::stderr(), "{} {:<5} {}: {}", timestamp(), record.level(), record.target(), record.args());
    }

    fn flush(&self) {}
}

/// `2026-10-27T09:30:00.123Z`, from the system clock through chrono.
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    chrono::DateTime::from_timestamp(i64::try_from(now.as_secs()).unwrap_or(0), now.subsec_nanos())
        .map_or_else(|| "-".to_owned(), |t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Install the logger, honouring `RUST_LOG`. Safe to call once; a second call leaves the first logger in place.
pub fn install() {
    let spec = std::env::var("RUST_LOG").ok().filter(|s| !s.trim().is_empty());
    let filter = Filter::parse(spec.as_deref().unwrap_or(DEFAULT_FILTER));
    let max = filter.max();
    if log::set_boxed_logger(Box::new(Logger { filter })).is_ok() {
        log::set_max_level(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_filter_is_chatty_for_us_and_quiet_for_everyone_else() {
        let f = Filter::parse(DEFAULT_FILTER);
        assert_eq!(f.level_for("xmlcraft_syntax"), LevelFilter::Info);
        assert_eq!(f.level_for("xmlcraft"), LevelFilter::Info);
        assert_eq!(f.level_for("wgpu_core::device"), LevelFilter::Warn);
        assert_eq!(f.max(), LevelFilter::Info);
    }

    #[test]
    fn the_most_specific_directive_wins() {
        let f = Filter::parse("warn,xmlcraft*=info,xmlcraft_syntax=trace,xmlcraft_syntax::lexer=off");
        assert_eq!(f.level_for("xmlcraft_syntax"), LevelFilter::Trace);
        assert_eq!(f.level_for("xmlcraft_syntax::cst"), LevelFilter::Trace, "a module path prefix");
        assert_eq!(f.level_for("xmlcraft_syntax::lexer"), LevelFilter::Off);
        assert_eq!(f.level_for("xmlcraft_syntaxx"), LevelFilter::Info, "not a path prefix, so only the star matches");
        assert_eq!(f.max(), LevelFilter::Trace);
    }

    #[test]
    fn garbage_in_the_spec_is_skipped() {
        let f = Filter::parse(" ,warn, =debug, a=nonsense, =, b=INFO ,");
        assert_eq!(f.level_for("x"), LevelFilter::Warn);
        assert_eq!(f.level_for("b"), LevelFilter::Info);
        assert_eq!(f.level_for("a"), LevelFilter::Warn);
        assert_eq!(Filter::parse("").level_for("x"), LevelFilter::Error, "nothing named: errors only");
    }

    #[test]
    fn a_bare_target_name_logs_everything_from_it() {
        let f = Filter::parse("my_crate");
        assert_eq!(f.level_for("my_crate::x"), LevelFilter::Trace);
        assert_eq!(f.level_for("other"), LevelFilter::Error);
    }

    #[test]
    fn the_timestamp_is_utc_and_well_formed() {
        let t = timestamp();
        assert_eq!(t.len(), 24, "{t}");
        assert!(t.ends_with('Z') && t.as_bytes()[10] == b'T', "{t}");
    }
}
