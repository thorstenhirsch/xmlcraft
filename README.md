# XmlCraft

An open-source, clean-room XML editor in the spirit of Altova XMLSpy, written in Rust.

> **Status: pre-alpha (0.0.1).** The repository is being bootstrapped. There is no application to download yet, and nothing below is shipped until the roadmap says so.

## Goals for the first release (0.1.0)

- An XML editor with syntax colouring and the features of a modern text editor: auto-completion, auto-indentation, folding, multi-cursor, find and replace.
- XSD and DTD support, and validation against both.
- Generate an XSD or DTD from an XML file.
- Generate sample XML from an XSD or DTD.
- The most popular standard schemas ship with the app; any other referenced schema is downloaded automatically and cached.

JSON and YAML (validated with JSON Schema) follow right after the first release.

## Platforms

Windows (x64, arm64), macOS (Intel and Apple Silicon, one universal build) and Linux (x64, arm64).

## How it is built

XmlCraft follows the conventions of its sibling project [PdfCraft](https://github.com/storytold/pdfcraft): Rust 2024, egui/eframe, a layered workspace checked by `cargo xtask`, a never-panic policy, and headless automation next to the desktop app.

```sh
cargo xtask help
```

## Clean-room

XmlCraft is not affiliated with Altova GmbH. It is built from public specifications (W3C, OASIS, IETF) and black-box behaviour, never from Altova's software, files or assets. "XMLSpy" is a trademark of Altova GmbH.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Third-party material and its licences are listed in [NOTICE](NOTICE).
