//! Tool windows: dock panels that are not the editor, each with its form state in the app (not in a dialog), drawn in the frame
//! of `kit::tool_body` and `kit::tool_footer`. Which panel each one is, and where it docks, is in the panel table
//! (`workbench::Panel::spec`).

pub(crate) mod search;
