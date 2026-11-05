//! The drafts of JSON Schema and what differs between them.

/// A draft of the specification. The order is the order of the drafts, so `draft >= Draft::D07` reads "draft 7 or later".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Draft {
    D04,
    D06,
    D07,
    D2019_09,
    D2020_12,
}

impl Draft {
    /// The draft a `$schema` URI names (the trailing `#` does not matter), or none.
    pub fn from_uri(uri: &str) -> Option<Draft> {
        let uri = uri.trim_end_matches('#');
        let uri = uri.strip_prefix("https://").or_else(|| uri.strip_prefix("http://"))?;
        Some(match uri {
            "json-schema.org/draft-04/schema" => Draft::D04,
            "json-schema.org/draft-06/schema" => Draft::D06,
            "json-schema.org/draft-07/schema" => Draft::D07,
            "json-schema.org/draft/2019-09/schema" => Draft::D2019_09,
            "json-schema.org/draft/2020-12/schema" => Draft::D2020_12,
            _ => return None,
        })
    }

    /// The URI of the meta-schema, as the drafts write it in `$schema`.
    pub fn uri(self) -> &'static str {
        match self {
            Draft::D04 => "http://json-schema.org/draft-04/schema#",
            Draft::D06 => "http://json-schema.org/draft-06/schema#",
            Draft::D07 => "http://json-schema.org/draft-07/schema#",
            Draft::D2019_09 => "https://json-schema.org/draft/2019-09/schema",
            Draft::D2020_12 => "https://json-schema.org/draft/2020-12/schema",
        }
    }

    /// A short name for messages and tools: `04`, `06`, `07`, `2019-09`, `2020-12`.
    pub fn name(self) -> &'static str {
        match self {
            Draft::D04 => "04",
            Draft::D06 => "06",
            Draft::D07 => "07",
            Draft::D2019_09 => "2019-09",
            Draft::D2020_12 => "2020-12",
        }
    }

    pub fn from_name(name: &str) -> Option<Draft> {
        [Draft::D04, Draft::D06, Draft::D07, Draft::D2019_09, Draft::D2020_12].into_iter().find(|d| d.name() == name)
    }

    /// The keyword that names a schema (`id` in draft 4, `$id` since).
    pub(crate) fn id_keyword(self) -> &'static str {
        if self == Draft::D04 { "id" } else { "$id" }
    }

    /// In drafts up to 7 a `$ref` makes the other keywords of its object ignored.
    pub(crate) fn ref_hides_siblings(self) -> bool {
        self <= Draft::D07
    }

    /// `true` and `false` are schemas since draft 6.
    pub(crate) fn boolean_schemas(self) -> bool {
        self >= Draft::D06
    }

    /// `1.0` is an integer since draft 6.
    pub(crate) fn float_integers(self) -> bool {
        self >= Draft::D06
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schema_uri_names_its_draft() {
        assert_eq!(Draft::from_uri("http://json-schema.org/draft-07/schema#"), Some(Draft::D07));
        assert_eq!(Draft::from_uri("http://json-schema.org/draft-07/schema"), Some(Draft::D07));
        assert_eq!(Draft::from_uri("https://json-schema.org/draft/2020-12/schema"), Some(Draft::D2020_12));
        assert_eq!(Draft::from_uri("http://json-schema.org/draft-03/schema#"), None);
        assert_eq!(Draft::from_uri("urn:x"), None);
        for d in [Draft::D04, Draft::D06, Draft::D07, Draft::D2019_09, Draft::D2020_12] {
            assert_eq!(Draft::from_uri(d.uri()), Some(d));
            assert_eq!(Draft::from_name(d.name()), Some(d));
        }
        assert_eq!(Draft::from_name("2019"), None);
        assert!(Draft::D07 < Draft::D2019_09);
    }
}
