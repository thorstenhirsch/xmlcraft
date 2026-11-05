# xmlcraft-convert

Conversions between XML, JSON, YAML and BSON (its MongoDB Extended JSON text), layer L4 (no UI, no I/O, compiles for the web).

- **Formats** (`Format`): `xml`, `json`, `yaml`, `bson`. `convert(text, from, to, &ConvertOptions)` returns the text and the `warnings`: what the target could not hold exactly, said once each (NaN and infinity in JSON, BSON types written as plain values, repeated names, a list of fewer than two items in XML, characters XML cannot hold). A list becomes several BSON documents, one to a line; JSON Lines (`ConvertOptions::json_lines`) are read as a list.
- **Data formats** are read into the value tree of `xmlcraft-json-syntax` (`read_value`) and written from it (`json_text`, `yaml_text`), so JSON ⇄ YAML ⇄ BSON are reading and writing. The YAML writer quotes every string that would read back as something else (`yes`, `1e3`, `---`, a leading `#`, …); several YAML documents or BSON documents become an array (YAML aliases are expanded into copies, which is not reported).
- **XML ⇄ data, natural mapping** (`xml_to_value`, `value_to_xml`; `XmlToData`, `DataToXml`): an element is a member; attributes are `@name`; text beside attributes or children is `#text`; repeated elements are an array; an empty element is `null`; comments and processing instructions are left out. Going back, names that are not XML names are repaired, an array is repeated elements, a namespace prefix is declared where `@xmlns:p` says and nowhere else, and data that XML cannot hold (a list as an attribute, a list in a list) is refused with the place.
- **XML ⇄ JSON, standard mapping** (`standard_to_xml`, `standard_to_value`): the `fn:json-to-xml` / `fn:xml-to-json` mapping of XPath 3.1, lossless for JSON.
- **BSON** (`extended_to_plain`): converting from the Extended JSON text to another format turns typed values into plain ones (`{"$numberInt": "1"}` into `1`, a date into its ISO string); converting to it reads plain numbers as int32, int64 or double by size.
- Never panics; depth and sizes are bounded, and every refusal says what and where.

Tests: `tests/convert.rs` (each mapping, what is lost, refusals; properties: JSON survives YAML and the standard XML mapping, any text converts or is refused without a panic, any data becomes well-formed XML). The tool is tested in `crates/automation/tests/convert.rs`.
