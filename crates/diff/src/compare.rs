//! The differences between two trees.

use std::collections::{HashMap, VecDeque};
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
    /// The parent of the node in the first text, and in the second (for an addition, the parent in the first text is the one that
    /// corresponds to the parent in the second, and the other way round for a removal: where it would go). Empty for a change of the
    /// root, which has no parent.
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

/// Where a node is in each document: its path, and its parent.
#[derive(Clone, Copy)]
struct Place<'a> {
    path: &'a str,
    parent: Option<&'a Node>,
}

/// Compare two trees (made by [`Node::from_xml`], [`Node::from_json`] or [`Node::from_yaml`]; both of the same family).
pub fn compare_trees(mut old: Node, mut new: Node, options: &TreeOptions) -> Vec<Change> {
    old.seal(options);
    new.seal(options);
    let xml = matches!(old.kind, NodeKind::Document) || matches!(new.kind, NodeKind::Document);
    let mut ctx = Ctx { options, xml, changes: Vec::new() };
    let root_path = if xml { "/".to_owned() } else { String::new() };
    if old.hash != new.hash {
        let place = Place { path: &root_path, parent: None };
        ctx.pair(&old, &new, place, place);
    }
    ctx.changes
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

impl Ctx<'_> {
    /// Names the nodes of a list of siblings by their place among them.
    fn labels(&self, parent: &Node, nodes: &[&Node]) -> Vec<String> {
        let mut seen: HashMap<(NodeKind, &str), usize> = HashMap::new();
        nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if !self.xml {
                    return if parent.kind == NodeKind::Array { i.to_string() } else { escape_pointer(&n.name) };
                }
                let count = seen.entry((n.kind, n.key.as_str())).and_modify(|c| *c += 1).or_insert(1);
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

    fn join(path: &str, label: &str) -> String {
        format!("{}/{label}", path.trim_end_matches('/'))
    }

    /// Record a difference. `old_at` and `new_at` are where the node is in each document (none: it is not there; the parent of the
    /// place is then where it would go).
    fn push(&mut self, kind: ChangeKind, path: String, old_at: (Option<&Node>, Option<&Node>), new_at: (Option<&Node>, Option<&Node>)) {
        let span = |n: Option<&Node>| n.map(|n| n.span.clone());
        self.changes.push(Change {
            kind,
            path,
            old: span(old_at.0),
            new: span(new_at.0),
            old_parent: span(old_at.1).unwrap_or(0..0),
            new_parent: span(new_at.1).unwrap_or(0..0),
            old_text: old_at.0.map(shown),
            new_text: new_at.0.map(shown),
        });
    }

    /// Two nodes that stand for each other, with different hashes.
    fn pair(&mut self, old: &Node, new: &Node, old_place: Place<'_>, new_place: Place<'_>) {
        if old.kind != new.kind || old.kind.is_leaf() {
            self.push(ChangeKind::Changed, new_place.path.to_owned(), (Some(old), old_place.parent), (Some(new), new_place.parent));
        } else {
            self.children(old, new, old_place.path, new_place.path);
        }
    }

    fn children(&mut self, old: &Node, new: &Node, old_path: &str, new_path: &str) {
        let (old_keyed, old_ordered): (Vec<&Node>, Vec<&Node>) =
            old.children.iter().partition(|c| c.kind == NodeKind::Attribute || old.kind == NodeKind::Object);
        let (new_keyed, new_ordered): (Vec<&Node>, Vec<&Node>) =
            new.children.iter().partition(|c| c.kind == NodeKind::Attribute || new.kind == NodeKind::Object);
        self.keyed(old, new, &old_keyed, &new_keyed, old_path, new_path);
        self.ordered(old, new, &old_ordered, &new_ordered, old_path, new_path);
    }

    fn removed(&mut self, node: &Node, label: &str, old: &Node, new: &Node, old_path: &str) {
        self.push(ChangeKind::Removed, Self::join(old_path, label), (Some(node), Some(old)), (None, Some(new)));
    }

    fn added(&mut self, node: &Node, label: &str, old: &Node, new: &Node, new_path: &str) {
        self.push(ChangeKind::Added, Self::join(new_path, label), (None, Some(old)), (Some(node), Some(new)));
    }

    /// Attributes and the members of objects: found by name (the n-th of a repeated name by its number), in no order.
    fn keyed(&mut self, old: &Node, new: &Node, olds: &[&Node], news: &[&Node], old_path: &str, new_path: &str) {
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
            let old_label = old_labels.get(i).map_or("", String::as_str);
            let found = old_ids.get(i).and_then(|id| index.get(id)).copied();
            match found.and_then(|j| news.get(j).map(|n| (j, *n))) {
                Some((j, n)) => {
                    if let Some(u) = used.get_mut(j) {
                        *u = true;
                    }
                    if o.hash != n.hash {
                        let (op, np) = (Self::join(old_path, old_label), Self::join(new_path, new_labels.get(j).map_or("", String::as_str)));
                        self.pair(o, n, Place { path: &op, parent: Some(old) }, Place { path: &np, parent: Some(new) });
                    }
                }
                None => self.removed(o, old_label, old, new, old_path),
            }
        }
        for (j, n) in news.iter().enumerate() {
            if used.get(j) == Some(&false) {
                self.added(n, new_labels.get(j).map_or("", String::as_str), old, new, new_path);
            }
        }
    }

    /// Children in their order: the longest run of equal ones is kept, and between those the ones of the same sort stand for each other
    /// (or, when the order does not matter, equal ones are found wherever they are).
    fn ordered(&mut self, old: &Node, new: &Node, olds: &[&Node], news: &[&Node], old_path: &str, new_path: &str) {
        let old_labels = self.labels(old, olds);
        let new_labels = self.labels(new, news);
        let unordered = self.options.ignore_child_order;
        let mut stands: Vec<Option<usize>> = vec![None; olds.len()];
        let mut taken = vec![false; news.len()];
        let pairs: Vec<(usize, usize)> = if unordered {
            // The first unused equal one of the second list.
            let mut by_hash: HashMap<u128, VecDeque<usize>> = HashMap::new();
            for (j, n) in news.iter().enumerate() {
                by_hash.entry(n.hash).or_default().push_back(j);
            }
            olds.iter().enumerate().filter_map(|(i, o)| by_hash.get_mut(&o.hash).and_then(VecDeque::pop_front).map(|j| (i, j))).collect()
        } else {
            let a: Vec<u128> = olds.iter().map(|n| n.hash).collect();
            let b: Vec<u128> = news.iter().map(|n| n.hash).collect();
            matches(&a, &b, self.options.budget)
        };
        for (i, j) in pairs {
            if let (Some(s), Some(t)) = (stands.get_mut(i), taken.get_mut(j)) {
                *s = Some(j);
                *t = true;
            }
        }
        // Between the equal ones, those of the same sort stand for each other: in one pass, with the unused ones of the second list in a
        // queue for each sort.
        let mut queues: HashMap<(NodeKind, &str), VecDeque<usize>> = HashMap::new();
        for (j, n) in news.iter().enumerate() {
            if taken.get(j) == Some(&false) {
                queues.entry((n.kind, n.key.as_str())).or_default().push_back(j);
            }
        }
        // Where the next equal one of the second list is, after each of the first.
        let mut limit = vec![news.len(); olds.len()];
        let mut upcoming = news.len();
        for i in (0..olds.len()).rev() {
            if let Some(slot) = limit.get_mut(i) {
                *slot = upcoming;
            }
            if let Some(Some(j)) = stands.get(i) {
                upcoming = *j;
            }
        }
        let mut floor = 0;
        for (i, o) in olds.iter().enumerate() {
            if let Some(Some(j)) = stands.get(i) {
                floor = j + 1;
                continue;
            }
            let Some(queue) = queues.get_mut(&(o.kind, o.key.as_str())) else { continue };
            let ceiling = limit.get(i).copied().unwrap_or(news.len());
            let chosen = if unordered {
                queue.pop_front()
            } else {
                while queue.front().is_some_and(|&j| j < floor) {
                    queue.pop_front();
                }
                match queue.front() {
                    Some(&j) if j < ceiling => queue.pop_front(),
                    _ => None,
                }
            };
            if let Some(j) = chosen {
                if let (Some(s), Some(t)) = (stands.get_mut(i), taken.get_mut(j)) {
                    *s = Some(j);
                    *t = true;
                }
                floor = j + 1;
            }
        }
        // Report in the order of the first document, with the additions where they stand in the second.
        let mut next_new = 0;
        for (i, o) in olds.iter().enumerate() {
            let old_label = old_labels.get(i).map_or("", String::as_str);
            match stands.get(i).copied().flatten() {
                Some(j) => {
                    while next_new < j {
                        if taken.get(next_new) == Some(&false)
                            && let Some(n) = news.get(next_new)
                        {
                            self.added(n, new_labels.get(next_new).map_or("", String::as_str), old, new, new_path);
                        }
                        next_new += 1;
                    }
                    next_new = next_new.max(j + 1);
                    if let Some(n) = news.get(j)
                        && o.hash != n.hash
                    {
                        let (op, np) = (Self::join(old_path, old_label), Self::join(new_path, new_labels.get(j).map_or("", String::as_str)));
                        self.pair(o, n, Place { path: &op, parent: Some(old) }, Place { path: &np, parent: Some(new) });
                    }
                }
                None => self.removed(o, old_label, old, new, old_path),
            }
        }
        for j in next_new..news.len() {
            if taken.get(j) == Some(&false)
                && let Some(n) = news.get(j)
            {
                self.added(n, new_labels.get(j).map_or("", String::as_str), old, new, new_path);
            }
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
