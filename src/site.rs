use crate::markdown;
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

pub struct Node {
    pub id: usize,
    pub parent: Option<usize>,
    pub title: String,
    pub has_heading: bool,
    pub source: Option<String>,
    pub route: String,
    pub folder: bool,
    pub children: Vec<usize>,
}

pub struct Site {
    pub nodes: Vec<Node>,
    pub assets: Vec<String>,
    pub targets: HashMap<String, String>,
}

impl Site {
    pub fn discover(root: &Path) -> Result<Self> {
        let mut site = Self {
            nodes: Vec::new(),
            assets: Vec::new(),
            targets: HashMap::new(),
        };
        let name = root.file_name().and_then(|s| s.to_str()).unwrap_or("Home");
        site.add_folder(root, "", name, None)?;
        let mut routes = HashMap::new();
        let mut outputs = HashMap::new();
        for node in &site.nodes {
            let source = node.source.as_deref().unwrap_or(&node.route);
            if let Some(previous) = routes.insert(&node.route, source) {
                bail!(
                    "route collision: {previous:?} and {source:?} both produce /{}",
                    node.route
                );
            }
            outputs.insert(format!("{}index.html", node.route), source.to_owned());
        }
        for asset in &site.assets {
            if let Some(previous) = outputs.insert(asset.clone(), asset.clone()) {
                bail!("output collision: {previous:?} and {asset:?}");
            }
        }
        // A copied file cannot also be a parent directory of another output.
        for (output, source) in &outputs {
            for (index, _) in output.match_indices('/') {
                if let Some(previous) = outputs.get(&output[..index]) {
                    bail!("output collision: {previous:?} and {source:?}");
                }
            }
        }
        let order: Vec<_> = site
            .nodes
            .iter()
            .map(|n| (n.title.to_lowercase(), n.route.clone()))
            .collect();
        for node in &mut site.nodes {
            node.children.sort_by(|a, b| order[*a].cmp(&order[*b]));
        }
        Ok(site)
    }

    fn add_folder(
        &mut self,
        root: &Path,
        relative: &str,
        name: &str,
        parent: Option<usize>,
    ) -> Result<()> {
        let id = self.nodes.len();
        let route = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/")
        };
        self.targets.insert(relative.to_owned(), route.clone());
        self.nodes.push(Node {
            id,
            parent,
            title: label(name),
            has_heading: false,
            source: None,
            route,
            folder: true,
            children: Vec::new(),
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(id);
        }
        let mut entries = fs::read_dir(root.join(relative))
            .with_context(|| format!("cannot read folder {relative:?}"))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let filename = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 filename in {relative:?}"))?;
            if filename.starts_with(['.', '_']) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = if relative.is_empty() {
                filename.clone()
            } else {
                format!("{relative}/{filename}")
            };
            if kind.is_dir() {
                self.add_folder(root, &path, &filename, Some(id))?;
            } else if kind.is_file() {
                if let Some(stem) = filename.strip_suffix(".md") {
                    let text = fs::read_to_string(entry.path())
                        .with_context(|| format!("cannot read {path}"))?;
                    let heading = markdown::title(&text);
                    if filename == "README.md" {
                        self.targets
                            .insert(path.clone(), self.nodes[id].route.clone());
                        self.nodes[id].source = Some(path);
                        self.nodes[id].has_heading = heading.is_some();
                        if let Some(title) = heading {
                            self.nodes[id].title = title;
                        }
                    } else {
                        let page_route = format!("{}{stem}/", self.nodes[id].route);
                        self.targets.insert(path.clone(), page_route.clone());
                        let child = self.nodes.len();
                        self.nodes[id].children.push(child);
                        self.nodes.push(Node {
                            id: child,
                            parent: Some(id),
                            title: heading.clone().unwrap_or_else(|| label(stem)),
                            has_heading: heading.is_some(),
                            source: Some(path),
                            route: page_route,
                            folder: false,
                            children: Vec::new(),
                        });
                    }
                } else {
                    self.targets.insert(path.clone(), path.clone());
                    self.assets.push(path);
                }
            }
        }
        Ok(())
    }

    pub fn ancestors(&self, id: usize) -> HashSet<usize> {
        let mut result = HashSet::new();
        let mut current = Some(id);
        while let Some(id) = current {
            result.insert(id);
            current = self.nodes[id].parent;
        }
        result
    }
}

fn label(name: &str) -> String {
    name.replace(['-', '_'], " ")
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn relative_url(from: &str, to: &str) -> String {
    let from: Vec<_> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<_> = to.split('/').filter(|s| !s.is_empty()).collect();
    // The last component of a file target is not a shared directory.
    let directories = if to.ends_with('/') || to.is_empty() {
        to_parts.len()
    } else {
        to_parts.len().saturating_sub(1)
    };
    let common = from
        .iter()
        .zip(&to_parts)
        .take(directories)
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = "../".repeat(from.len() - common);
    for (i, part) in to_parts[common..].iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        for byte in part.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                result.push(byte as char);
            } else {
                use std::fmt::Write;
                write!(result, "%{byte:02X}").unwrap();
            }
        }
    }
    if to.ends_with('/') && !result.is_empty() && !result.ends_with('/') {
        result.push('/');
    }
    if result.is_empty() {
        result.push_str("./");
    }
    // Prevent a filename containing ':' from being mistaken for a URL scheme (encoded above).
    result
}
