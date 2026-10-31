use crate::site::{Site, relative_url};
use anyhow::{Result, bail};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use std::collections::HashSet;

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

pub fn title(text: &str) -> Option<String> {
    let mut inside = false;
    let mut title = String::new();
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) => inside = true,
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if inside => return Some(title),
            Event::Text(value) | Event::Code(value) if inside => title.push_str(&value),
            Event::SoftBreak | Event::HardBreak if inside => title.push(' '),
            _ => (),
        }
    }
    None
}

pub fn render(text: &str, source: &str, route: &str, site: &Site) -> Result<String> {
    let mut events: Vec<_> = Parser::new_ext(text, options()).collect();
    let mut ids = HashSet::from(["main".to_owned()]);
    for index in 0..events.len() {
        if matches!(events[index], Event::Start(Tag::Heading { .. })) {
            let mut heading = String::new();
            for event in &events[index + 1..] {
                match event {
                    Event::End(TagEnd::Heading(_)) => break,
                    Event::Text(value) | Event::Code(value) => heading.push_str(value),
                    Event::SoftBreak | Event::HardBreak => heading.push(' '),
                    _ => (),
                }
            }
            let base = slug(&heading);
            let mut id = base.clone();
            let mut suffix = 1;
            while !ids.insert(id.clone()) {
                id = format!("{base}-{suffix}");
                suffix += 1;
            }
            if let Event::Start(Tag::Heading { id: value, .. }) = &mut events[index] {
                *value = Some(id.into());
            }
        }
        if let Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) =
            &mut events[index]
        {
            *dest_url = rewrite(dest_url, source, route, site)?.into();
        }
    }
    let mut output = String::with_capacity(text.len() * 2);
    html::push_html(&mut output, events.into_iter());
    Ok(output)
}

fn slug(text: &str) -> String {
    let mut result = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            result.push(ch);
        } else if ch.is_whitespace() && !result.ends_with('-') {
            result.push('-');
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        "section".to_owned()
    } else {
        result.to_owned()
    }
}

fn rewrite(url: &str, source: &str, route: &str, site: &Site) -> Result<String> {
    let scheme = url.split_once(':').is_some_and(|(prefix, _)| {
        prefix.starts_with(|c: char| c.is_ascii_alphabetic())
            && prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if url.is_empty() || url.starts_with(['#', '?']) || url.starts_with("//") || scheme {
        return Ok(url.to_owned());
    }
    let split = url.find(['?', '#']).unwrap_or(url.len());
    let path = decode(&url[..split])?;
    let mut parts: Vec<_> = if path.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map(|(parent, _)| parent.split('/').collect())
            .unwrap_or_default()
    };
    for part in path.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                if parts.pop().is_none() {
                    bail!("local link escapes input folder: {url}");
                }
            }
            value => parts.push(value),
        }
    }
    let key = parts.join("/");
    let target = site
        .targets
        .get(&key)
        .ok_or_else(|| anyhow::anyhow!("unresolved local link: {url}"))?;
    Ok(format!("{}{}", relative_url(route, target), &url[split..]))
}

fn decode(text: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let a = input.next().and_then(|b| (b as char).to_digit(16));
            let b = input.next().and_then(|b| (b as char).to_digit(16));
            match (a, b) {
                (Some(a), Some(b)) => bytes.push((a * 16 + b) as u8),
                _ => bail!("invalid URL encoding: {text}"),
            }
        } else {
            bytes.push(byte);
        }
    }
    Ok(String::from_utf8(bytes)?)
}
