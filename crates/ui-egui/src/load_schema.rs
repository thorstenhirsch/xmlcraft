//! Load Schema: open the schema or the DTD the document in front names, in a tab of its own; and what kind of document it is, which
//! decides whether the command bar offers Schema… (for XML) or XML… (for a schema).

use std::path::Path;

use xmlcraft_engine::doc_kind::{DocKind, doc_kind};
use xmlcraft_engine::schema_fetch::FetchJob;
use xmlcraft_engine::schema_link::{SchemaLink, schema_link};

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};

impl XmlCraftApp {
    /// What the document in front is.
    pub(crate) fn active_doc_kind(&self) -> Option<DocKind> {
        let doc = self.session.active_document()?;
        Some(doc_kind(doc.name(), self.active_analysis()))
    }

    /// The grammar the document in front names. Judged from the last analysis, which may be of slightly older text: a command that
    /// turned grey while the text is being typed would flicker.
    pub(crate) fn active_schema_link(&self) -> Option<SchemaLink> {
        schema_link(self.active_analysis()?)
    }

    /// Ask for the schema the document in front names, and open it when it has been found.
    pub(crate) fn load_schema(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let version = doc.editor().version();
        let base = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let current = self.active_analysis().is_some_and(|a| a.version == version);
        let Some(link) = self.active_schema_link() else {
            self.status = tl!("This document names no schema.").to_owned();
            return;
        };
        if !current {
            // The link may have been edited since: wait for the analysis of this text.
            self.status = tl!("The document is still being read. Try again in a moment.").to_owned();
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.loading_schema = Some(serial);
        self.resolution_serial = None;
        self.status = fmt(tl!("Loading {name}…"), &[("name", &link.label())]);
        self.schema_fetcher.submit(FetchJob { serial, link, base, open: self.open_documents() });
    }

    /// Ask how the schema the document in front names is looked up, and show every place the resolver tries.
    pub(crate) fn show_schema_resolution(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let version = doc.editor().version();
        let base = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let current = self.active_analysis().is_some_and(|a| a.version == version);
        let Some(link) = self.active_schema_link() else {
            self.status = tl!("This document names no schema.").to_owned();
            return;
        };
        if !current {
            self.status = tl!("The document is still being read. Try again in a moment.").to_owned();
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.resolution_serial = Some(serial);
        self.loading_schema = None;
        self.status = fmt(tl!("Resolving {name}…"), &[("name", &link.label())]);
        self.schema_fetcher.submit(FetchJob { serial, link, base, open: self.open_documents() });
    }

    /// Collect what the fetcher found and open it. Called every frame.
    pub(crate) fn pump_schema_fetch(&mut self) {
        for outcome in self.schema_fetcher.poll() {
            if self.resolution_serial == Some(outcome.serial) {
                self.resolution_serial = None;
                let answer = match &outcome.result {
                    Ok(found) => fmt(tl!("Found in {origin}: {location}"), &[("origin", tl!(found.origin_name())), ("location", &found.location)]),
                    Err(message) => message.clone(),
                };
                let text = fmt(
                    tl!("{link}\n\n{trail}\n\n{answer}"),
                    &[("link", &outcome.link.label()), ("trail", &outcome.trail.to_string()), ("answer", &answer)],
                );
                self.information(tl!("Schema Resolution"), text);
                continue;
            }
            if self.loading_schema != Some(outcome.serial) {
                continue; // asked for before the last request
            }
            self.loading_schema = None;
            match outcome.result {
                Ok(found) => {
                    let name = found.location.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("schema").to_owned();
                    let opened = match &found.path {
                        Some(path) if Path::new(path).is_file() => self.open_path(Path::new(path)),
                        _ => {
                            self.open_generated(&name, &found.text);
                            true
                        }
                    };
                    if opened {
                        self.status = fmt(tl!("Opened {name} from {origin}"), &[("name", &name), ("origin", found.origin_name())]);
                    }
                }
                Err(message) => self.message(tl!("Couldn't load the schema"), message),
            }
        }
    }
}
