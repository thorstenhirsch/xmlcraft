//! What HTML5 is made of: its elements (which are void, which have raw text, which may leave their end tag out, which are obsolete), the
//! attributes each takes and the values some attributes list. Written from the WHATWG HTML Living Standard (a public specification).

/// What the reader needs to know of an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementInfo {
    pub name: &'static str,
    /// Has no content and no end tag (`<br>`).
    pub void: bool,
    /// Its content is text up to its end tag, not markup (`<script>`, `<style>`, `<textarea>`, `<title>`).
    pub raw_text: bool,
    /// Its end tag may be left out.
    pub optional_end: bool,
    /// Obsolete in HTML5: browsers render it, authors should not write it.
    pub obsolete: bool,
}

const fn el(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: false, raw_text: false, optional_end: false, obsolete: false }
}

const fn void(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: true, raw_text: false, optional_end: false, obsolete: false }
}

const fn raw(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: false, raw_text: true, optional_end: false, obsolete: false }
}

const fn opt(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: false, raw_text: false, optional_end: true, obsolete: false }
}

const fn old(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: false, raw_text: false, optional_end: false, obsolete: true }
}

const fn old_void(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: true, raw_text: false, optional_end: false, obsolete: true }
}

const fn old_raw(name: &'static str) -> ElementInfo {
    ElementInfo { name, void: false, raw_text: true, optional_end: false, obsolete: true }
}

/// Every element, sorted by name.
pub const ELEMENTS: &[ElementInfo] = &[
    el("a"),
    el("abbr"),
    old("acronym"),
    el("address"),
    old("applet"),
    void("area"),
    el("article"),
    el("aside"),
    el("audio"),
    el("b"),
    void("base"),
    old_void("basefont"),
    el("bdi"),
    el("bdo"),
    old_void("bgsound"),
    old("big"),
    old("blink"),
    el("blockquote"),
    opt("body"),
    void("br"),
    el("button"),
    el("canvas"),
    opt("caption"),
    old("center"),
    el("cite"),
    el("code"),
    void("col"),
    opt("colgroup"),
    el("data"),
    el("datalist"),
    opt("dd"),
    el("del"),
    el("details"),
    el("dfn"),
    el("dialog"),
    old("dir"),
    el("div"),
    el("dl"),
    opt("dt"),
    el("em"),
    void("embed"),
    el("fieldset"),
    el("figcaption"),
    el("figure"),
    old("font"),
    el("footer"),
    el("form"),
    old_void("frame"),
    old("frameset"),
    el("h1"),
    el("h2"),
    el("h3"),
    el("h4"),
    el("h5"),
    el("h6"),
    opt("head"),
    el("header"),
    el("hgroup"),
    void("hr"),
    opt("html"),
    el("i"),
    raw("iframe"),
    void("img"),
    void("input"),
    el("ins"),
    el("kbd"),
    old_void("keygen"),
    el("label"),
    el("legend"),
    opt("li"),
    void("link"),
    old_raw("listing"),
    el("main"),
    el("map"),
    el("mark"),
    old("marquee"),
    el("math"),
    el("menu"),
    old_void("menuitem"),
    void("meta"),
    el("meter"),
    el("nav"),
    old("nobr"),
    old_raw("noembed"),
    old_raw("noframes"),
    el("noscript"),
    el("object"),
    el("ol"),
    opt("optgroup"),
    opt("option"),
    el("output"),
    opt("p"),
    void("param"),
    el("picture"),
    old_raw("plaintext"),
    el("pre"),
    el("progress"),
    el("q"),
    old("rb"),
    opt("rp"),
    opt("rt"),
    old("rtc"),
    el("ruby"),
    el("s"),
    el("samp"),
    raw("script"),
    el("search"),
    el("section"),
    el("select"),
    el("slot"),
    el("small"),
    void("source"),
    el("span"),
    old("strike"),
    el("strong"),
    raw("style"),
    el("sub"),
    el("summary"),
    el("sup"),
    el("svg"),
    el("table"),
    opt("tbody"),
    opt("td"),
    el("template"),
    raw("textarea"),
    opt("tfoot"),
    opt("th"),
    opt("thead"),
    el("time"),
    raw("title"),
    opt("tr"),
    void("track"),
    old("tt"),
    el("u"),
    el("ul"),
    el("var"),
    el("video"),
    void("wbr"),
    old_raw("xmp"),
];

/// The element called `name` (lower case), if HTML has it.
pub fn element(name: &str) -> Option<&'static ElementInfo> {
    ELEMENTS.binary_search_by(|e| e.name.cmp(name)).ok().and_then(|i| ELEMENTS.get(i))
}

/// Whether `name` (lower case) is a void element.
pub fn is_void(name: &str) -> bool {
    element(name).is_some_and(|e| e.void)
}

/// A custom element's name (`my-widget`): lower case, starts with a letter and has a hyphen.
pub fn is_custom_element(name: &str) -> bool {
    name.contains('-') && name.starts_with(|c: char| c.is_ascii_lowercase()) && !name.chars().any(|c| c.is_ascii_uppercase() || c.is_whitespace())
}

/// The attributes every element takes.
pub const GLOBAL_ATTRIBUTES: &[&str] = &[
    "accesskey",
    "autocapitalize",
    "autofocus",
    "class",
    "contenteditable",
    "dir",
    "draggable",
    "enterkeyhint",
    "hidden",
    "id",
    "inert",
    "inputmode",
    "is",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "lang",
    "nonce",
    "popover",
    "role",
    "slot",
    "spellcheck",
    "style",
    "tabindex",
    "title",
    "translate",
    "writingsuggestions",
];

/// The attributes an element takes besides the global ones.
pub fn attributes_of(element: &str) -> &'static [&'static str] {
    match element {
        "a" => &["href", "target", "download", "ping", "rel", "hreflang", "type", "referrerpolicy"],
        "area" => &["alt", "coords", "shape", "href", "target", "download", "ping", "rel", "referrerpolicy"],
        "audio" => &["src", "crossorigin", "preload", "autoplay", "loop", "muted", "controls"],
        "base" => &["href", "target"],
        "blockquote" | "q" => &["cite"],
        "body" => &["onload", "onunload"],
        "button" => &[
            "disabled",
            "form",
            "formaction",
            "formenctype",
            "formmethod",
            "formnovalidate",
            "formtarget",
            "name",
            "popovertarget",
            "popovertargetaction",
            "type",
            "value",
        ],
        "canvas" => &["width", "height"],
        "col" | "colgroup" => &["span"],
        "data" => &["value"],
        "del" | "ins" => &["cite", "datetime"],
        "details" => &["open", "name"],
        "dialog" => &["open"],
        "embed" => &["src", "type", "width", "height"],
        "fieldset" => &["disabled", "form", "name"],
        "form" => &["accept-charset", "action", "autocomplete", "enctype", "method", "name", "novalidate", "target", "rel"],
        "html" => &["manifest"],
        "iframe" => &["src", "srcdoc", "name", "sandbox", "allow", "allowfullscreen", "width", "height", "referrerpolicy", "loading"],
        "img" => &[
            "alt",
            "src",
            "srcset",
            "sizes",
            "crossorigin",
            "usemap",
            "ismap",
            "width",
            "height",
            "referrerpolicy",
            "decoding",
            "loading",
            "fetchpriority",
        ],
        "input" => &[
            "accept",
            "alt",
            "autocomplete",
            "checked",
            "dirname",
            "disabled",
            "form",
            "formaction",
            "formenctype",
            "formmethod",
            "formnovalidate",
            "formtarget",
            "height",
            "list",
            "max",
            "maxlength",
            "min",
            "minlength",
            "multiple",
            "name",
            "pattern",
            "placeholder",
            "popovertarget",
            "readonly",
            "required",
            "size",
            "src",
            "step",
            "type",
            "value",
            "width",
        ],
        "label" => &["for"],
        "li" => &["value"],
        "link" => &[
            "href",
            "crossorigin",
            "rel",
            "media",
            "hreflang",
            "type",
            "sizes",
            "as",
            "imagesrcset",
            "imagesizes",
            "referrerpolicy",
            "integrity",
            "blocking",
            "fetchpriority",
        ],
        "map" => &["name"],
        "meta" => &["name", "http-equiv", "content", "charset", "media"],
        "meter" => &["value", "min", "max", "low", "high", "optimum"],
        "object" => &["data", "type", "name", "form", "width", "height"],
        "ol" => &["reversed", "start", "type"],
        "optgroup" => &["disabled", "label"],
        "option" => &["disabled", "label", "selected", "value"],
        "output" => &["for", "form", "name"],
        "progress" => &["value", "max"],
        "script" => &["src", "type", "nomodule", "async", "defer", "crossorigin", "integrity", "referrerpolicy", "blocking", "fetchpriority"],
        "select" => &["autocomplete", "disabled", "form", "multiple", "name", "required", "size"],
        "slot" => &["name"],
        "source" => &["type", "src", "srcset", "sizes", "media", "width", "height"],
        "style" => &["media", "blocking"],
        "td" => &["colspan", "rowspan", "headers"],
        "template" => &["shadowrootmode", "shadowrootdelegatesfocus", "shadowrootclonable"],
        "textarea" => &[
            "autocomplete",
            "cols",
            "dirname",
            "disabled",
            "form",
            "maxlength",
            "minlength",
            "name",
            "placeholder",
            "readonly",
            "required",
            "rows",
            "wrap",
        ],
        "th" => &["colspan", "rowspan", "headers", "scope", "abbr"],
        "time" => &["datetime"],
        "track" => &["default", "kind", "label", "src", "srclang"],
        "video" => &["src", "crossorigin", "preload", "autoplay", "loop", "muted", "controls", "poster", "width", "height", "playsinline"],
        _ => &[],
    }
}

/// Whether `name` is an attribute that any element may have though it is not listed: `data-*`, `aria-*` and the event handlers (`on…`).
pub fn is_open_attribute(name: &str) -> bool {
    name.starts_with("data-")
        || name.starts_with("aria-")
        || (name.starts_with("on") && name.len() > 2 && name.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The values an attribute of an element lists (empty: it takes any).
pub fn attribute_values(element: &str, attribute: &str) -> &'static [&'static str] {
    match (element, attribute) {
        ("input", "type") => &[
            "button",
            "checkbox",
            "color",
            "date",
            "datetime-local",
            "email",
            "file",
            "hidden",
            "image",
            "month",
            "number",
            "password",
            "radio",
            "range",
            "reset",
            "search",
            "submit",
            "tel",
            "text",
            "time",
            "url",
            "week",
        ],
        ("button", "type") => &["submit", "reset", "button"],
        ("form", "method") => &["get", "post", "dialog"],
        ("form" | "button" | "input", "formmethod") => &["get", "post", "dialog"],
        ("form", "enctype") | ("button" | "input", "formenctype") => &["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"],
        ("a" | "area" | "base" | "form", "target") | ("button" | "input", "formtarget") => &["_self", "_blank", "_parent", "_top"],
        (_, "dir") => &["ltr", "rtl", "auto"],
        (_, "draggable" | "spellcheck") => &["true", "false"],
        (_, "contenteditable") => &["true", "false", "plaintext-only"],
        (_, "translate") => &["yes", "no"],
        (_, "autocapitalize") => &["none", "off", "sentences", "on", "words", "characters"],
        (_, "inputmode") => &["none", "text", "decimal", "numeric", "tel", "search", "email", "url"],
        (_, "popover") => &["auto", "manual", "hint"],
        ("audio" | "video", "preload") => &["none", "metadata", "auto"],
        ("img" | "iframe", "loading") => &["lazy", "eager"],
        ("img", "decoding") => &["sync", "async", "auto"],
        ("img" | "link" | "script" | "audio" | "video", "crossorigin") => &["anonymous", "use-credentials"],
        (_, "fetchpriority") => &["high", "low", "auto"],
        (_, "referrerpolicy") => &[
            "no-referrer",
            "no-referrer-when-downgrade",
            "origin",
            "origin-when-cross-origin",
            "same-origin",
            "strict-origin",
            "strict-origin-when-cross-origin",
            "unsafe-url",
        ],
        ("th", "scope") => &["row", "col", "rowgroup", "colgroup"],
        ("track", "kind") => &["subtitles", "captions", "descriptions", "chapters", "metadata"],
        ("ol", "type") => &["1", "a", "A", "i", "I"],
        ("textarea", "wrap") => &["soft", "hard"],
        ("form" | "input" | "select" | "textarea", "autocomplete") => &["on", "off"],
        ("script", "type") => &["module", "importmap", "speculationrules", "text/javascript"],
        ("meta", "name") => &["viewport", "description", "keywords", "author", "robots", "theme-color", "color-scheme", "referrer", "generator"],
        ("meta", "http-equiv") => &["content-type", "default-style", "refresh", "x-ua-compatible", "content-security-policy"],
        ("link", "rel") => &[
            "stylesheet",
            "icon",
            "preload",
            "prefetch",
            "preconnect",
            "dns-prefetch",
            "modulepreload",
            "canonical",
            "alternate",
            "manifest",
            "next",
            "prev",
            "author",
            "license",
            "help",
            "search",
        ],
        ("a" | "area" | "form", "rel") => {
            &["noopener", "noreferrer", "nofollow", "external", "help", "license", "next", "prev", "search", "tag", "author", "bookmark", "alternate"]
        }
        ("link", "as") => &["audio", "document", "embed", "fetch", "font", "image", "object", "script", "style", "track", "video", "worker"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_element_table_is_sorted_without_repeats_so_that_the_lookup_works() {
        assert!(ELEMENTS.windows(2).all(|w| w[0].name < w[1].name), "sorted and unique");
        for name in ["br", "p", "script", "svg", "td"] {
            assert!(element(name).is_some(), "{name}");
        }
        assert!(element("nothing").is_none() && element("").is_none());
    }

    #[test]
    fn void_raw_text_and_optional_end_tags_are_what_the_standard_says() {
        let voids: Vec<&str> = ELEMENTS.iter().filter(|e| e.void && !e.obsolete).map(|e| e.name).collect();
        assert_eq!(voids, ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"]);
        assert!(
            element("script").is_some_and(|e| e.raw_text)
                && element("title").is_some_and(|e| e.raw_text)
                && element("textarea").is_some_and(|e| e.raw_text)
        );
        for name in ["li", "p", "td", "tr", "option", "body"] {
            assert!(element(name).is_some_and(|e| e.optional_end), "{name}");
        }
        assert!(element("font").is_some_and(|e| e.obsolete) && !element("div").is_some_and(|e| e.obsolete));
    }

    #[test]
    fn custom_elements_have_a_hyphen_and_open_attributes_are_data_aria_and_events() {
        assert!(is_custom_element("my-widget") && !is_custom_element("widget") && !is_custom_element("My-Widget"));
        assert!(is_open_attribute("data-x") && is_open_attribute("aria-label") && is_open_attribute("onclick"));
        assert!(!is_open_attribute("on") && !is_open_attribute("href") && !is_open_attribute("onClick"));
    }

    #[test]
    fn attributes_and_their_values_are_listed_for_the_elements_that_take_them() {
        assert!(attributes_of("a").contains(&"href") && attributes_of("input").contains(&"type") && attributes_of("div").is_empty());
        assert!(attribute_values("input", "type").contains(&"checkbox") && attribute_values("a", "target").contains(&"_blank"));
        assert!(attribute_values("div", "dir") == ["ltr", "rtl", "auto"] && attribute_values("div", "id").is_empty());
    }
}
