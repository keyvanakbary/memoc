use crate::site::{Node, Site, escape, relative_url};
use std::{collections::HashSet, fmt::Write};

pub fn page(site: &Site, node: &Node, body: &str) -> String {
    let title = escape(&node.title);
    let site_title = escape(&site.nodes[0].title);
    let ancestors = site.ancestors(node.id);
    let mut navigation = String::new();
    nav(site, 0, node, &ancestors, &mut navigation);
    let mut breadcrumbs = Vec::new();
    let mut parent = node.parent;
    while let Some(id) = parent {
        breadcrumbs.push(link(&site.nodes[id], &node.route, false));
        parent = site.nodes[id].parent;
    }
    breadcrumbs.reverse();
    let mut content = String::new();
    if !node.has_heading {
        writeln!(content, "<h1>{title}</h1>").unwrap();
    }
    content.push_str(body);
    if node.folder {
        content.push_str(
            "<section class=\"children\" aria-label=\"Contents\"><h2>Contents</h2><ul>\n",
        );
        for child in &node.children {
            writeln!(
                content,
                "<li>{}</li>",
                link(&site.nodes[*child], &node.route, false)
            )
            .unwrap();
        }
        content.push_str("</ul></section>\n");
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title} · {site_title}</title>\n<link rel=\"stylesheet\" href=\"{}\">\n</head>\n<body>\n<a class=\"skip\" href=\"#main\">Skip to content</a>\n<aside>\n<a class=\"site-title\" href=\"{}\">{site_title}</a>\n<nav class=\"desktop-nav\" aria-label=\"Site\">{navigation}</nav>\n<details class=\"mobile-nav\"><summary>Navigation</summary><nav aria-label=\"Site\">{navigation}</nav></details>\n</aside>\n<main id=\"main\" tabindex=\"-1\">\n<nav class=\"breadcrumbs\" aria-label=\"Breadcrumbs\">{}</nav>\n{content}</main>\n</body>\n</html>\n",
        escape(&relative_url(&node.route, "_memoc.css")),
        escape(&relative_url(&node.route, "")),
        breadcrumbs.join(" <span aria-hidden=\"true\">/</span> ")
    )
}

fn link(node: &Node, from: &str, active: bool) -> String {
    format!(
        "<a href=\"{}\"{}>{}</a>",
        escape(&relative_url(from, &node.route)),
        if active { " aria-current=\"page\"" } else { "" },
        escape(&node.title)
    )
}

fn nav(site: &Site, folder: usize, current: &Node, ancestors: &HashSet<usize>, out: &mut String) {
    out.push_str("<ul>");
    for id in &site.nodes[folder].children {
        let node = &site.nodes[*id];
        write!(
            out,
            "<li>{}",
            link(node, &current.route, node.id == current.id)
        )
        .unwrap();
        // Expand only the current branch; other folders remain reachable through their landing pages.
        if node.folder && ancestors.contains(id) && !node.children.is_empty() {
            nav(site, *id, current, ancestors, out);
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>");
}
