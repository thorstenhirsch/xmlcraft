//! The neutral invoice model on the synthetic fixtures: the same invoice in UBL and in CII gives the same model (apart from where each
//! value is), every value points at its text, and the large and broken cases are bounded.

use xmlcraft_einvoice::Syntax;
use xmlcraft_einvoice::model::{Decimal, Invoice, MAX_LINES, read};

const UBL: &str = include_str!("fixtures/xrechnung-ubl.xml");
const CII: &str = include_str!("fixtures/xrechnung-cii.xml");

/// Every field of the model by its path, in the neutral form (dates as ISO dates), without the places.
fn values(invoice: &Invoice) -> Vec<(String, String, Option<Decimal>, Option<String>)> {
    let mut fields = Vec::new();
    invoice.tree().flatten("", &mut fields);
    fields.into_iter().map(|(path, f)| (path, f.normalized(), f.decimal, f.unit.clone())).collect()
}

#[test]
fn the_same_invoice_in_ubl_and_in_cii_gives_the_same_model() {
    let ubl = read(UBL).expect("a UBL invoice");
    let cii = read(CII).expect("a CII invoice");
    assert_eq!((ubl.syntax, cii.syntax), (Syntax::Ubl, Syntax::Cii));
    let (u, c) = (values(&ubl), values(&cii));
    // The paths that differ, so a failure says where.
    let only_ubl: Vec<_> = u.iter().filter(|v| !c.contains(v)).collect();
    let only_cii: Vec<_> = c.iter().filter(|v| !u.contains(v)).collect();
    assert!(only_ubl.is_empty() && only_cii.is_empty(), "only in UBL: {only_ubl:#?}\nonly in CII: {only_cii:#?}");
    assert_eq!(u, c);
    assert!(ubl.cut.is_empty() && cii.cut.is_empty(), "{:?} {:?}", ubl.cut, cii.cut);
}

#[test]
fn the_model_has_what_the_invoice_says() {
    let inv = read(CII).expect("an invoice");
    let text = |path: &str| inv.lookup(path).map(|f| f.normalized());
    assert_eq!(text("number").as_deref(), Some("RE-2026-0001"));
    assert_eq!(text("type_code").as_deref(), Some("380"));
    assert_eq!(text("issue_date").as_deref(), Some("2026-11-01"));
    assert_eq!(text("due_date").as_deref(), Some("2026-12-01"));
    assert_eq!(text("delivery.date").as_deref(), Some("2026-10-30"));
    assert_eq!(text("currency").as_deref(), Some("EUR"));
    assert_eq!(text("buyer.reference").as_deref(), Some("04011000-12345-67"));
    assert_eq!(text("buyer_reference").as_deref(), Some("04011000-12345-67"));
    assert_eq!(text("seller.name").as_deref(), Some("Beispiel Lieferant GmbH"));
    assert_eq!(text("seller.address.line1").as_deref(), Some("Musterstrasse 1"));
    assert_eq!(text("seller.address.postcode").as_deref(), Some("10115"));
    assert_eq!(text("seller.vat_id").as_deref(), Some("DE123456789"));
    assert_eq!(text("seller.legal_id").as_deref(), Some("HRB 12345"));
    assert_eq!(text("seller.contact.email").as_deref(), Some("erika@example.com"));
    assert_eq!(text("seller.electronic_address").as_deref(), Some("rechnung@example.com"));
    assert_eq!(text("buyer.address.city").as_deref(), Some("Hamburg"));
    assert_eq!(text("payment.iban").as_deref(), Some("DE02120300000000202051"));
    assert_eq!(text("payment.means_code").as_deref(), Some("58"));
    assert_eq!(text("payment_terms").as_deref(), Some("Zahlbar innerhalb von 30 Tagen."));
    assert_eq!(text("totals.payable").as_deref(), Some("119.00"));
    assert_eq!(text("vat_breakdown.1.rate").as_deref(), Some("19"));
    assert_eq!(text("lines.1.name").as_deref(), Some("Beratung"));
    assert_eq!(text("lines.1.unit").as_deref(), Some("C62"));
    assert_eq!(text("lines.2.name"), None);
    assert_eq!(text("seller.nothing"), None);
    assert_eq!(text("lines"), None, "a list is no field");
    let payable = inv.totals.payable.as_ref().expect("payable");
    assert_eq!((payable.decimal, payable.unit.as_deref()), (Some(Decimal { mantissa: 11900, scale: 2 }), Some("EUR")), "the invoice currency");
    let tax = inv.totals.tax_total.as_ref().expect("tax total");
    assert_eq!(tax.unit.as_deref(), Some("EUR"));
    assert_eq!(inv.lines_total, 1);
}

#[test]
fn every_value_points_at_its_text() {
    for source in [UBL, CII] {
        let inv = read(source).expect("an invoice");
        let mut fields = Vec::new();
        inv.tree().flatten("", &mut fields);
        assert!(fields.len() > 30, "{}", fields.len());
        for (path, f) in fields {
            assert_eq!(source.get(f.range.clone()), Some(f.text.as_str()), "{path}");
        }
    }
}

#[test]
fn a_credit_note_has_its_own_names() {
    let note = UBL
        .replace("xsd:Invoice-2", "xsd:CreditNote-2")
        .replace("ubl:Invoice", "ubl:CreditNote")
        .replace("<cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>", "<cbc:CreditNoteTypeCode>381</cbc:CreditNoteTypeCode>")
        .replace("cac:InvoiceLine", "cac:CreditNoteLine")
        .replace("cbc:InvoicedQuantity", "cbc:CreditedQuantity");
    let inv = read(&note).expect("a credit note");
    assert_eq!(inv.document, "Credit note");
    assert_eq!(inv.lookup("type_code").map(|f| f.text).as_deref(), Some("381"));
    assert_eq!(inv.lookup("lines.1.quantity").map(|f| f.text).as_deref(), Some("2"));
}

#[test]
fn lines_past_the_limit_are_counted_not_read() {
    let line = UBL.split("<cac:InvoiceLine>").nth(1).and_then(|l| l.split("</cac:InvoiceLine>").next()).expect("a line");
    let many = format!("<cac:InvoiceLine>{line}</cac:InvoiceLine>").repeat(MAX_LINES + 3);
    let doc = UBL.replace("</ubl:Invoice>", &format!("{many}</ubl:Invoice>"));
    let inv = read(&doc).expect("an invoice");
    assert_eq!((inv.lines.len(), inv.lines_total), (MAX_LINES, MAX_LINES + 4));
    assert!(inv.cut.iter().any(|c| c.contains("more lines")), "{:?}", inv.cut);
}

#[test]
fn a_document_that_is_cut_off_still_gives_what_it_has() {
    let at = CII.find("<ram:ApplicableHeaderTradeDelivery>").expect("there");
    let inv = read(&CII[..at]).expect("an invoice, in part");
    assert_eq!(inv.lookup("number").map(|f| f.text).as_deref(), Some("RE-2026-0001"));
    assert_eq!(inv.totals.payable, None);
}
