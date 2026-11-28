use serde_json::Value;
use std::collections::{HashMap, HashSet};

const MAX_TREE_BYTES: usize = 40 * 1024;

fn value_text(node: &Value, key: &str) -> String {
    node.get(key)
        .and_then(|value| value.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn actionable(role: &str) -> bool {
    matches!(
        role,
        "link"
            | "button"
            | "textbox"
            | "searchbox"
            | "combobox"
            | "listbox"
            | "option"
            | "checkbox"
            | "radio"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "tab"
            | "switch"
            | "slider"
            | "spinbutton"
            | "treeitem"
    )
}

fn meaningful(role: &str) -> bool {
    matches!(
        role,
        "heading"
            | "statictext"
            | "paragraph"
            | "listitem"
            | "image"
            | "cell"
            | "rowheader"
            | "columnheader"
            | "definition"
            | "emphasis"
            | "strong"
    )
}

struct Render<'a> {
    nodes: &'a HashMap<String, &'a Value>,
    output: String,
    refs: HashMap<String, i64>,
    visiting: HashSet<String>,
    next_ref: usize,
    truncated: bool,
}

impl Render<'_> {
    fn push_line(&mut self, depth: usize, line: String) {
        if self.truncated {
            return;
        }
        let line = format!("{}{line}\n", "  ".repeat(depth));
        if self.output.len() + line.len() > MAX_TREE_BYTES {
            self.output.push_str("\n… snapshot truncated\n");
            self.truncated = true;
            return;
        }
        self.output.push_str(&line);
    }

    fn node(&mut self, id: &str, depth: usize) {
        if self.truncated || !self.visiting.insert(id.to_owned()) {
            return;
        }
        let Some(node) = self.nodes.get(id).copied() else {
            self.visiting.remove(id);
            return;
        };

        let role = value_text(node, "role").to_ascii_lowercase();
        let name = value_text(node, "name");
        let ignored = node.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let is_actionable = actionable(&role);
        let include = !ignored
            && (is_actionable || (!name.is_empty() && meaningful(&role)) || role == "rootwebarea");

        let child_depth = if include {
            let mut line = if role.is_empty() { "node".to_owned() } else { role.clone() };
            if !name.is_empty() {
                line.push_str(&format!(" \"{}\"", name.replace('\\', "\\\\").replace('"', "\\\"")));
            }
            if is_actionable {
                if let Some(backend_node) = node.get("backendDOMNodeId").and_then(Value::as_i64) {
                    let reference = format!("e{}", self.next_ref);
                    self.next_ref += 1;
                    self.refs.insert(reference.clone(), backend_node);
                    line.push_str(&format!(" [ref={reference}]"));
                }
            }
            self.push_line(depth, line);
            depth + 1
        } else {
            depth
        };

        if let Some(children) = node.get("childIds").and_then(Value::as_array) {
            for child in children.iter().filter_map(Value::as_str) {
                self.node(child, child_depth);
            }
        }
        self.visiting.remove(id);
    }
}

/// Renders the result of Accessibility.getFullAXTree and returns the ref ->
/// backend node map that actions use until the next snapshot.
pub fn render_ax_tree(result: &Value) -> Result<(String, HashMap<String, i64>), String> {
    let nodes_value = result
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "Accessibility.getFullAXTree returned no nodes".to_owned())?;
    let nodes = nodes_value
        .iter()
        .filter_map(|node| {
            node.get("nodeId")
                .and_then(Value::as_str)
                .map(|id| (id.to_owned(), node))
        })
        .collect::<HashMap<_, _>>();
    if nodes.is_empty() {
        return Err("Accessibility.getFullAXTree returned an empty tree".to_owned());
    }

    let mut children = HashSet::new();
    for node in nodes_value {
        if let Some(ids) = node.get("childIds").and_then(Value::as_array) {
            children.extend(ids.iter().filter_map(Value::as_str));
        }
    }
    let mut roots = nodes
        .keys()
        .filter(|id| !children.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if roots.is_empty() {
        roots.push(nodes.keys().next().expect("non-empty node map").clone());
    }

    let mut render = Render {
        nodes: &nodes,
        output: String::new(),
        refs: HashMap::new(),
        visiting: HashSet::new(),
        next_ref: 1,
        truncated: false,
    };
    for root in roots {
        render.node(&root, 0);
    }
    Ok((render.output, render.refs))
}
