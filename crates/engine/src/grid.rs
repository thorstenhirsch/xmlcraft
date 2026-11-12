//! The grid of a document: its text read as the tree table of `xmlcraft-grid`, as its language says (JSON Lines as one array of their
//! lines, a BSON file's Extended JSON with its wrappers as values).

use xmlcraft_grid::{Flavor, Grid, GridError};

use crate::compare::family_of;
use crate::language::{JsonFlavor, Language};

/// How the grid reads a text of `language` beyond its family.
pub fn flavor_of(language: Language) -> Flavor {
    match language {
        Language::Json(JsonFlavor::Lines) => Flavor::JsonLines,
        Language::Bson => Flavor::ExtendedJson,
        _ => Flavor::Plain,
    }
}

/// The grid of `text`, written in `language`.
pub fn read_grid(text: &str, language: Language) -> Result<Grid, GridError> {
    Grid::read_flavored(text, family_of(language), flavor_of(language))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lines_and_bson_are_read_as_their_flavor() {
        assert!(read_grid("{\"a\": 1}\n{\"a\": 2}\n", Language::Json(JsonFlavor::Lines)).is_ok());
        assert!(read_grid("{\"a\": 1}\n{\"a\": 2}\n", Language::Json(JsonFlavor::Json)).is_err(), "two values are not one JSON text");
        let ejson = "[{\"n\": {\"$numberLong\": \"5\"}}, {\"n\": {\"$numberLong\": \"6\"}}]";
        let grid = read_grid(ejson, Language::Bson).expect("reads");
        assert_eq!(grid.table(ejson, &[]).map(|t| t.rows[0].cells[0].text.clone()), Some("5".to_owned()));
    }
}
