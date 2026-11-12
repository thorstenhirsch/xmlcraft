# xmlcraft-invoice-pdf

Layer L4. The visual rendering of an electronic invoice as a PDF that carries the original XML unchanged: a ZUGFeRD 2.x /
Factur-X hybrid for a UN/CEFACT CII invoice. Tool: `einvoice_render_pdf`.

- `render(xml, bytes, &Options)`: reads the invoice into the neutral model of `xmlcraft-einvoice`, lays it out with a template and
  writes the PDF. `bytes` (the file's own bytes) are embedded as they are.
- Writing goes through PdfCraft's object layer, `pdfcraft-cos` (git dependency on `storytold/pdfcraft` at a pinned revision, with
  PdfCraft's patched `hayro-jbig2` patched in the same way in the root manifest). Text is set in Inter Regular and SemiBold
  (`assets/fonts/`, OFL), embedded as subsets (Type 0 / CIDFontType2, Identity-H, `/ToUnicode`).
- The embedding follows the Factur-X 1.07.2 specification (= ZUGFeRD 2.3) §6.2–6.3: `factur-x.xml` (`xrechnung.xml` for the
  XRECHNUNG reference profile), `text/xml`, `/Params /ModDate`, `/AF` in the catalog and the `/EmbeddedFiles` name tree,
  `/AFRelationship` per the table of §6.2.2 (Data for MINIMUM and BASIC WL, Alternative otherwise), and the `fx:` XMP extension
  schema with its PDF/A description. A UBL invoice is attached as `invoice.xml` with `/AFRelationship /Source`; Factur-X is CII only.
- **PDF/A-3:** the file identifies itself as PDF/A-3b and carries an sRGB output intent: the built-in compact v2 sRGB profile
  (`assets/icc/sRGB-v2-micro.icc`, CC0-1.0, saucecontrol/Compact-ICC-Profiles) unless the caller passes another output or display
  profile (`Options::icc_profile`). **Nothing here verifies PDF/A or Factur-X conformance** (veraPDF, Mustang): check the output
  externally.

## Templates

```xml
<invoice-template version="1" page="A4" margin="20mm" font-size="9.5" lang="de">
  <header> … </header>   <!-- on every page -->
  <body> … </body>
  <footer> … </footer>   <!-- on every page; {page} and {pages} are known here -->
</invoice-template>
```

Blocks (each takes `if="path"`, hidden when the model has no value there, and `top="45mm"`, the least distance from the top of the
first page it starts at):

- `<block style="title|heading|normal|small|bold" align="left|center|right">text {path|filter}</block>`; an empty `<block/>` is a
  blank line; a block whose placeholders are all empty is left out; separators ` · ` around empty values close up.
- `<columns widths="55% 45%"><column>…</column>…</columns>`
- `<fields><field label="…" value="{…}" if="…"/>…</fields>`: a label and value list (a field with an empty value is left out).
- `<table rows="lines|vat_breakdown|allowances_charges|notes" widths="…"><col title="…" value="{…}" sub="{…}" align="right"/>…</table>`:
  breaks across pages and repeats its header row.
- `<totals/>`: the totals the invoice has, the amount due in bold.

Placeholders address the model (`seller.name`, `buyer.address.city`, `totals.payable`; in a table, the row's fields such as `name`,
`net_price`), plus `document_name` (Rechnung / Gutschrift, Invoice / Credit note). Filters: `date`, `money`, `percent`, `number`,
`unit` (common UN/ECE Rec. 20 codes as words), `upper`. A path that names nothing of the model renders empty and is a warning; a
template mistake is an error with its line and column. Bounds: 1 MiB template, nesting 8, 5,000 blocks, 2,000 pages.

The built-in `de` (DIN 5008 form B address window) and `en` templates are in `templates/`.
