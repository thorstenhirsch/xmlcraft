# Screenshots

| File | Shows |
|---|---|
| [`editor.png`](editor.png) | The editor: a document, its outline, and Entry Helpers offering what its XML Schema allows at the caret |
| [`validation.png`](validation.png) | Validation (F8) against the schema the document names: the problems underlined and listed (dark theme) |
| [`generate-schema.png`](generate-schema.png) | XML ▸ Generate Schema…: an XML Schema or a DTD learned from the open documents |
| [`new-from-template.png`](new-from-template.png) | File ▸ New from Template… |

They are pictures of the real window, drawn by the app's own headless renderer from the synthetic documents in
[`fixtures/`](fixtures) (a book catalogue and its schema; `library-errors.xml` has mistakes on purpose). Regenerate them with
`packaging/screenshots.sh`. Nothing in them comes from any other software (`AGENTS.md` §1); the provenance is in
[`LICENSE.txt`](LICENSE.txt) and `ATTRIBUTION.toml`.
