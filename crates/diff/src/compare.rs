//! The differences between two trees.

use std::collections::HashMap;
use std::ops::Range;

use crate::node::{Node, NodeKind, TreeOptions};
use crate::seq::matches;

/// What happened to a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// Only in the second document.
    Added,
    /// Only in the first document.
    Removed,
    /// In both, with a different value (a leaf) or a different kind.
    Changed,
}

/// One difference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    /// Where: for XML an XPath-like path (`/root/item[2]/@id`, `/root/item[2]/text()[1]`), for JSON and YAML a JSON Pointer
    /// (`/items/1/id`). It is the path in the second document for what is added or changed, and in the first for what is removed.
    pub path: String,
    /// The node in the first text (none for an addition) …
    pub old: Option<Range<usize>>,
    /// … and in the second (none for a removal).
    pub new: Option<Range<usize>>,
    /// Where the parent is in the first text (what an addition goes into) and in the second (what a removal came out of).
    pub old_parent: Range<usize>,
    pub new_parent: Range<usize>,
    /// What the node was and is: the value of a leaf, the name of an element, `{…}` or `[…]` for a container.
    pub old_text: Option<String>,
    pub new_text: Option<String>,
}

impl Change {
    /// A line for a list: `changed /a/b: 1 → 2`.
    pub fn describe(&self) -> String {
        let show = |t: &Option<String>| t.clone().unwrap_or_default();
        match self.kind {
            ChangeKind::Added => format!("added {}: {}", self.path, show(&self.new_text)),
            ChangeKind::Removed => format!("removed {}: {}", self.path, show(&self.old_text)),
            ChangeKind::Changed => format!("changed {}: {} → {}", self.path, show(&self.old_text), show(&self.new_text)),
        }
    }
}

/// The most characters of a value that a [`Change`] repeats.
const SHOWN: usize = 200;

fn shown(n: &Node) -> String {
    let mut text = match n.kind {
        NodeKind::Document | NodeKind::Object => "{…}".to_owned(),
        NodeKind::Array => "[…]".to_owned(),
        NodeKind::Element => format!("<{}>", n.name),
        NodeKind::String => format!("{:?}", n.text),
        NodeKind::ProcessingInstruction => format!("<?{} {}?>", n.name, n.text),
        NodeKind::Comment => format!("<!--{}-->", n.text),
        _ => n.text.clone(),
    };
    if text.chars().count() > SHOWN {
        text = text.chars().take(SHOWN).chain(std::iter::once('…')).collect();
    }
    text
}

struct Ctx<'a> {
    options: &'a TreeOptions,
    xml: bool,
    changes: Vec<Change>,
}

/// Compare two trees (made by [`Node::from_xml`], [`Node::from_json`] or [`Node::from_yaml`]; both of the same family).
pub fn compare_trees(mut old: Node, mut new: Node, options: &TreeOptions) -> Vec<Change> {
    old.seal(options);
    new.seal(options);
    let xml = matches!(old.kind, NodeKind::Document) || matches!(new.kind, NodeKind::Document);
    let mut ctx = Ctx { options, xml, changes: Vec::new() };
    let root_path = if xml { "/".to_owned() } else { String::new() };
    if old.hash != new.hash {
        ctx.pair(&old, &new, &root_path);
    }
    ctx.changes
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

impl Ctx<'_> {
    /// Names the nodes of a list of siblings by their place among them.
    fn labels(&self, parent: &Node, nodes: &[&Node]) -> Vec<String> {
        let mut seen: HashMap<(NodeKind, String), usize> = HashMap::new();
        nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if !self.xml {
                    return if parent.kind == NodeKind::Array { i.to_string() } else { escape_pointer(&n.name) };
                }
                let count = seen.entry((n.kind, n.key.clone())).and_modify(|c| *c += 1).or_insert(1);
                match n.kind {
                    NodeKind::Element => format!("{}[{count}]", n.name),
                    NodeKind::Attribute => format!("@{}", n.name),
                    NodeKind::Text => format!("text()[{count}]"),
                    NodeKind::Comment => format!("comment()[{count}]"),
                    NodeKind::ProcessingInstruction => format!("processing-instruction('{}')[{count}]", n.name),
                    _ => i.to_string(),
                }
            })
            .collect()
    }

    fn join(&self, path: &str, label: &str) -> String {
        format!("{}/{label}", path.trim_end_matches('/'))
    }

    fn push(&mut self, kind: ChangeKind, path: String, old: Option<(&Node, &Node)>, new: Option<(&Node, &Node)>) {
        let span = |p: &Option<(&Node, &Node)>| p.map(|(n, _)| n.span.clone());
        let parent = |p: &Option<(&Node, &Node)>| p.map(|(_, parent)| parent.span.clone());
        let (old_parent, new_parent) = match (old, new) {
            (Some(o), Some(n)) => (parent(&Some(o)).unwrap_or(0..0), parent(&Some(n)).unwrap_or(0..0)),
            (Some(o), None) => (parent(&Some(o)).unwrap_or(0..0), 0..0),
            (None, Some(n)) => (0..0, parent(&Some(n)).unwrap_or(0..0)),
            (None, None) => (0..0, 0..0),
        };
        self.changes.push(Change {
            kind,
            path,
            old: span(&old),
            new: span(&new),
            old_parent,
            new_parent,
            old_text: old.map(|(n, _)| shown(n)),
            new_text: new.map(|(n, _)| shown(n)),
        });
    }

    /// Two nodes that stand for each other, with different hashes.
    fn pair(&mut self, old: &Node, new: &Node, path: &str) {
        if old.kind != new.kind || old.kind.is_leaf() {
            self.push(ChangeKind::Changed, path.to_owned(), Some((old, old)), Some((new, new)));
        } else {
            self.children(old, new, path);
        }
    }

    fn children(&mut self, old: &Node, new: &Node, path: &str) {
        let (old_keyed, old_ordered): (Vec<&Node>, Vec<&Node>) =
            old.children.iter().partition(|c| c.kind == NodeKind::Attribute || old.kind == NodeKind::Object);
        let (new_keyed, new_ordered): (Vec<&Node>, Vec<&Node>) =
            new.children.iter().partition(|c| c.kind == NodeKind::Attribute || new.kind == NodeKind::Object);
        self.keyed(old, new, &old_keyed, &new_keyed, path);
        self.ordered(old, new, &old_ordered, &new_ordered, path);
    }

    /// Attributes and the members of objects: found by name (the n-th of a repeated name by its number), in no order.
    fn keyed(&mut self, old: &Node, new: &Node, olds: &[&Node], news: &[&Node], path: &str) {
        let old_labels = self.labels(old, olds);
        let new_labels = self.labels(new, news);
        let numbered = |nodes: &[&Node]| -> Vec<(String, usize)> {
            let mut seen: HashMap<&str, usize> = HashMap::new();
            nodes
                .iter()
                .map(|n| {
                    let k = seen.entry(n.key.as_str()).and_modify(|c| *c += 1).or_insert(0);
                    (n.key.clone(), *k)
                })
                .collect()
        };
        let (old_ids, new_ids) = (numbered(olds), numbered(news));
        let index: HashMap<&(String, usize), usize> = new_ids.iter().enumerate().map(|(i, k)| (k, i)).collect();
        let mut used = vec![false; news.len()];
        for (i, o) in olds.iter().enumerate() {
            let found = old_ids.get(i).and_then(|id| index.get(id)).copied();
            match found.and_then(|j| news.get(j).map(|n| (j, *n))) {
                Some((j, n)) => {
                    if let Some(u) = used.get_mut(j) {
                        *u = true;
                    }
                    if o.hash != n.hash {
                        let label = new_labels.get(j).cloned().unwrap_or_default();
                        let p = self.join(path, &label);
                        self.pair_in(o, n, old, new, &p);
                    }
                }
                None => {
                    let p = self.join(path, old_labels.get(i).map_or("", String::as_str));
                    self.push(ChangeKind::Removed, p, Some((o, old)), None);
                    if let Some(c) = self.changes.last_mut() {
                        c.new_parent = new.span.clone();
                    }
                }
            }
        }
        for (j, n) in news.iter().enumerate() {
            if used.get(j) == Some(&false) {
                let p = self.join(path, new_labels.get(j).map_or("", String::as_str));
                self.push(ChangeKind::Added, p, None, Some((n, new)));
                if let Some(c) = self.changes.last_mut() {
                    c.old_parent = old.span.clone();
                }
            }
        }
    }

    /// `pair` with the parents known (for the places of the parents).
    fn pair_in(&mut self, o: &Node, n: &Node, old_parent: &Node, new_parent: &Node, path: &str) {
        let before = self.changes.len();
        self.pair(o, n, path);
        for c in self.changes.get_mut(before..).unwrap_or_default() {
            if c.path == path {
                c.old_parent = old_parent.span.clone();
                c.new_parent = new_parent.span.clone();
            }
        }
    }

    /// Children in their order: the longest run of equal ones is kept, and between those the ones of the same sort stand for each other.
    fn ordered(&mut self, old: &Node, new: &Node, olds: &[&Node], news: &[&Node], path: &str) {
        let old_labels = self.labels(old, olds);
        let new_labels = self.labels(new, news);
        let mut stands: Vec<Option<usize>> = vec![None; olds.len()];
        let mut taken = vec![false; news.len()];
        let pairs: Vec<(usize, usize)> = if self.options.ignore_child_order {
            // Equal ones are found wherever they are; the first unused equal one of the second list.
            let mut by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
            for (j, n) in news.iter().enumerate() {
                by_hash.entry(n.hash).or_default().push(j);
            }
            for v in by_hash.values_mut() {
                v.reverse();
            }
            olds.iter().enumerate().filter_map(|(i, o)| by_hash.get_mut(&o.hash).and_then(Vec::pop).map(|j| (i, j))).collect()
        } else {
            let a: Vec<u64> = olds.iter().map(|n| n.hash).collect();
            let b: Vec<u64> = news.iter().map(|n| n.hash).collect();
            matches(&a, &b, self.options.budget)
        };
        for &(i, j) in &pairs {
            if let (Some(s), Some(t)) = (stands.get_mut(i), taken.get_mut(j)) {
                *s = Some(j);
                *t = true;
            }
        }
        // Between the equal ones, those of the same sort (in order, when the order matters) stand for each other.
        let same_sort = |a: &Node, b: &Node| a.kind == b.kind && a.key == b.key;
        let mut floor = 0;
        for i in 0..olds.len() {
            if let Some(Some(j)) = stands.get(i) {
                floor = j + 1;
                continue;
            }
            let Some(o) = olds.get(i) else { continue };
            let limit = stands.iter().skip(i + 1).flatten().min().copied().unwrap_or(news.len());
            let range = if self.options.ignore_child_order { 0..news.len() } else { floor..limit.max(floor) };
            let candidate = range.into_iter().find(|&j| taken.get(j) == Some(&false) && news.get(j).is_some_and(|n| same_sort(o, n)));
            if let Some(j) = candidate {
                if let (Some(s), Some(t)) = (stands.get_mut(i), taken.get_mut(j)) {
                    *s = Some(j);
                    *t = true;
                }
                if !self.options.ignore_child_order {
                    floor = j + 1;
                }
            }
        }
        // Report in the order of the first document, with the additions where they stand in the second.
        let mut reported_new = vec![false; news.len()];
        let mut next_new = 0;
        for (i, o) in olds.iter().enumerate() {
            match stands.get(i).copied().flatten() {
                Some(j) => {
                    // Additions before it.
                    while next_new < j {
                        if taken.get(next_new) == Some(&false) && reported_new.get(next_new) == Some(&false) {
                            self.add(new_labels.get(next_new), news.get(next_new), old, new, path);
                            if let Some(r) = reported_new.get_mut(next_new) {
                                *r = true;
                            }
                        }
                        next_new += 1;
                    }
                    next_new = next_new.max(j + 1);
                    if let Some(n) = news.get(j)
                        && o.hash != n.hash
                    {
                        let p = self.join(path, new_labels.get(j).map_or("", String::as_str));
                        self.pair_in(o, n, old, new, &p);
                    }
                }
                None => {
                    let p = self.join(path, old_labels.get(i).map_or("", String::as_str));
                    self.push(ChangeKind::Removed, p, Some((o, old)), None);
                    if let Some(c) = self.changes.last_mut() {
                        c.new_parent = new.span.clone();
                    }
                }
            }
        }
        for j in 0..news.len() {
            if taken.get(j) == Some(&false) && reported_new.get(j) == Some(&false) {
                self.add(new_labels.get(j), news.get(j), old, new, path);
            }
        }
    }

    fn add(&mut self, label: Option<&String>, node: Option<&&Node>, old: &Node, new: &Node, path: &str) {
        let Some(n) = node else { return };
        let p = self.join(path, label.map_or("", String::as_str));
        self.push(ChangeKind::Added, p, None, Some((n, new)));
        if let Some(c) = self.changes.last_mut() {
            c.old_parent = old.span.clone();
        }
    }
}

/// What a text is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Xml,
    Json,
    Yaml,
}

impl Family {
    /// `xml`, `json` or `yaml`.
    pub fn parse(name: &str) -> Option<Family> {
        match name {
            "xml" => Some(Family::Xml),
            "json" => Some(Family::Json),
            "yaml" | "yml" => Some(Family::Yaml),
            _ => None,
        }
    }

    /// The tree of a text.
    pub fn read(self, text: &str) -> Result<Node, crate::node::DiffError> {
        match self {
            Family::Xml => Node::from_xml(text),
            Family::Json => Node::from_json(text),
            Family::Yaml => Node::from_yaml(text),
        }
    }
}

/// The differences between two texts of one family, as trees. A text that cannot be read is refused (its lines can still be compared).
pub fn compare_documents(old: &str, new: &str, family: Family, options: &TreeOptions) -> Result<Vec<Change>, crate::node::DiffError> {
    compare_mixed(old, family, new, family, options)
}

/// [`compare_documents`] for texts of different families: JSON and YAML read into the same trees, so a JSON file can be compared with a
/// YAML file. (XML against either is a difference of everything.)
pub fn compare_mixed(
    old: &str,
    old_family: Family,
    new: &str,
    new_family: Family,
    options: &TreeOptions,
) -> Result<Vec<Change>, crate::node::DiffError> {
    Ok(compare_trees(old_family.read(old)?, new_family.read(new)?, options))
}
