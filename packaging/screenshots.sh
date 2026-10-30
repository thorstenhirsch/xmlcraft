#!/bin/sh
# Renders the screenshots in docs/screenshots from the synthetic documents in docs/screenshots/fixtures, with the app's own
# headless renderer (no window opens). Run from anywhere; needs a Rust toolchain.
set -eu
cd "$(dirname "$0")/.."
out=docs/screenshots
fix=$out/fixtures
shot() {
    name=$1
    shift
    cargo run -q -p xmlcraft-ui-egui --example shot -- "$out/$name.png" --size 1440x900 --scale 2 --width 1200 "$@"
}
shot editor --theme light --open "$fix/library.xml" --goto 7:5
shot validation --theme dark --open "$fix/library-errors.xml" --command xml.validate
shot generate-schema --theme light --open "$fix/library.xml" --command xml.generate_schema
shot new-from-template --theme light --open "$fix/library.xml" --command file.new_from_template
