//! PDFs built by hand for the tests: a classic one with the invoice XML embedded, and a PDF 1.5 one with an object stream and a
//! cross-reference stream.
#![allow(dead_code)]

use xmlcraft_pdf::ObjRef;

pub const XML: &str = "<?xml version=\"1.0\"?><rsm:CrossIndustryInvoice xmlns:rsm=\"urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100\"/>";

pub fn r(n: u32) -> ObjRef {
    ObjRef { number: n, generation: 0 }
}

/// A PDF made of objects written one after the other, with a classic cross-reference table.
pub struct Builder {
    pub out: Vec<u8>,
    pub offsets: Vec<(u32, usize)>,
}

impl Builder {
    pub fn new() -> Self {
        Self { out: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(), offsets: Vec::new() }
    }

    pub fn object(&mut self, number: u32, body: &str) {
        self.offsets.push((number, self.out.len()));
        self.out.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    }

    pub fn stream(&mut self, number: u32, dict: &str, data: &[u8]) {
        self.offsets.push((number, self.out.len()));
        self.out.extend_from_slice(format!("{number} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes());
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    /// The table for the objects written since `from`, the trailer, and startxref.
    pub fn finish(mut self, size: u32, trailer_extra: &str) -> Vec<u8> {
        let table = self.out.len();
        self.out.extend_from_slice(b"xref\n");
        let mut sorted = self.offsets.clone();
        sorted.sort_unstable();
        // One subsection for object 0 and one for each run of numbers.
        self.out.extend_from_slice(b"0 1\n0000000000 65535 f \n");
        let mut i = 0;
        while i < sorted.len() {
            let mut j = i;
            while j + 1 < sorted.len() && sorted[j + 1].0 == sorted[j].0 + 1 {
                j += 1;
            }
            self.out.extend_from_slice(format!("{} {}\n", sorted[i].0, j - i + 1).as_bytes());
            for (_, offset) in &sorted[i..=j] {
                self.out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
            }
            i = j + 1;
        }
        self.out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R {trailer_extra} >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
        self.out
    }
}

pub fn invoice_pdf(name: &str, xml: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(invoice.xml) 4 0 R] >> >> /AF [4 0 R] >>");
    b.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.object(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>");
    b.object(4, &format!("<< /Type /Filespec /F ({name}) /UF ({name}) /AFRelationship /Data /Desc (The invoice) /EF << /F 5 0 R /UF 5 0 R >> >>"));
    b.stream(5, &format!("/Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size {} >>", xml.len()), xml.as_bytes());
    b.finish(6, "")
}

/// A PDF 1.7 file that uses what PDF 1.5 added: the catalog and the filespec are in an object stream, the cross-reference is a stream with a PNG predictor, and the
/// embedded file is deflated.
/// How `compressed_pdf_with` differs from `compressed_pdf`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Plain,
    /// The cross-reference stream puts the object stream inside itself.
    NestedObjectStream,
    /// `/Index` asks for more rows than the stream has.
    MissingRows,
    /// The entry for the filespec has the wrong index in its object stream.
    WrongIndex,
    /// The embedded stream (object 5) is generation 1.
    LaterGeneration,
    /// The object stream's `/Filter` is object 4, which is inside the object stream.
    FilterInsideItself,
}

pub fn compressed_pdf(xml: &str) -> Vec<u8> {
    compressed_pdf_with(xml, Variant::Plain)
}

pub fn compressed_pdf_with(xml: &str, variant: Variant) -> Vec<u8> {
    use miniz_oxide::deflate::compress_to_vec_zlib;
    let mut out = b"%PDF-1.7\n".to_vec();
    // The object stream (object 10) holds objects 1 (catalog) and 4 (filespec).
    let one = "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(i) 4 0 R] >> >> >>";
    let generation: u8 = if variant == Variant::LaterGeneration { 1 } else { 0 };
    let four = format!("<< /F (z.xml) /EF << /UF 5 {generation} R >> /AFRelationship /Source >>");
    let filter = if variant == Variant::FilterInsideItself { "4 0 R" } else { "/FlateDecode" };
    let header = format!("1 0 4 {} ", one.len() + 1);
    let body = format!("{header}{one} {four}");
    let packed = compress_to_vec_zlib(body.as_bytes(), 6);
    let objstm_at = out.len();
    out.extend_from_slice(
        format!("10 0 obj\n<< /Type /ObjStm /N 2 /First {} /Filter {filter} /Length {} >>\nstream\n", header.len(), packed.len()).as_bytes(),
    );
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let file = compress_to_vec_zlib(xml.as_bytes(), 6);
    let stream_at = out.len();
    out.extend_from_slice(
        format!("5 {generation} obj\n<< /Type /EmbeddedFile /Subtype /application#2Fxml /Filter /FlateDecode /Length {} >>\nstream\n", file.len())
            .as_bytes(),
    );
    out.extend_from_slice(&file);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    // The cross-reference stream (object 11): entries 0..=11, W [1 4 1], PNG "Up" predictor (12), 6 bytes a row.
    let table_at = out.len();
    let mut rows: Vec<[u8; 6]> = Vec::new();
    for n in 0..12u32 {
        let (kind, f1, f2): (u8, u32, u8) = match n {
            0 => (0, 0, 255),
            1 => (2, 10, 0),
            4 if variant == Variant::WrongIndex => (2, 10, 7),
            4 => (2, 10, 1),
            5 => (1, u32::try_from(stream_at).expect("fits"), generation),
            10 if variant == Variant::NestedObjectStream => (2, 10, 0),
            10 => (1, u32::try_from(objstm_at).expect("fits"), 0),
            11 => (1, u32::try_from(table_at).expect("fits"), 0),
            _ => (0, 0, 0),
        };
        let b = f1.to_be_bytes();
        rows.push([kind, b[0], b[1], b[2], b[3], f2]);
    }
    let mut predicted: Vec<u8> = Vec::new();
    let mut previous = [0u8; 6];
    for row in &rows {
        predicted.push(2); // Up
        for i in 0..6 {
            predicted.push(row[i].wrapping_sub(previous[i]));
        }
        previous = *row;
    }
    let packed = compress_to_vec_zlib(&predicted, 6);
    let index = if variant == Variant::MissingRows { "/Index [0 13]" } else { "" };
    out.extend_from_slice(
        format!(
            "11 0 obj\n<< /Type /XRef /Size 12 /W [1 4 1] {index} /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 6 >> /Length {} >>\nstream\n",
            packed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&packed);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{table_at}\n%%EOF\n").as_bytes());
    out
}
