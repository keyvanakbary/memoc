mod markdown;
mod render;
mod server;
mod site;
mod watch;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result, bail};
use std::{env, fs, path::Path, time::Instant};

const MARKER: &str = "memoc output v1\n";

fn main() {
    if let Err(error) = run() {
        eprintln!("memoc: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [arg] if arg == "--help" => {
            println!(
                "Usage: memoc build [folder]\n       memoc watch [folder]\n       memoc <folder>\n\nBuild plain Markdown into <folder>/_site/.\nREADME.md introduces a folder; its children are always listed.\nBoth commands default to the current folder.\n\n  build        Build once and exit\n  watch        Build, serve at http://127.0.0.1:4000, and rebuild on changes\n               Refresh the browser after edits.\n  --help       Show this help\n  --version    Show the version"
            );
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("memoc {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command] if command == "build" => build(Path::new(".")),
        [command, folder] if command == "build" && !folder.to_string_lossy().starts_with('-') => {
            build(Path::new(folder))
        }
        [command] if command == "watch" => watch::run(Path::new(".")),
        [command, folder] if command == "watch" && !folder.to_string_lossy().starts_with('-') => {
            watch::run(Path::new(folder))
        }
        [arg] if !arg.to_string_lossy().starts_with('-') => build(Path::new(arg)),
        _ => bail!("usage: memoc build [folder] or memoc watch [folder] (see --help)"),
    }
}

fn build(input: &Path) -> Result<()> {
    let started = Instant::now();
    let input = input
        .canonicalize()
        .with_context(|| format!("cannot open {}", input.display()))?;
    anyhow::ensure!(input.is_dir(), "{} is not a folder", input.display());
    let output = input.join("_site");
    check_output(&output)?;
    // A fixed lock also prevents two builders from replacing each other's output.
    let staging = input.join("_memoc-build");
    fs::create_dir(&staging).context("cannot create _memoc-build (another build may be running; remove a stale directory after an interrupted build)")?;
    let mut cleanup = Cleanup {
        path: staging.clone(),
        published: false,
    };
    let site = site::Site::discover(&input)?;
    let generated = staging.join("site");
    fs::create_dir(&generated)?;
    fs::write(generated.join("_memoc.css"), include_str!("style.css"))?;
    for node in &site.nodes {
        let body = match &node.source {
            Some(source) => {
                let text = fs::read_to_string(input.join(source))
                    .with_context(|| format!("cannot read {source}"))?;
                markdown::render(&text, source, &node.route, &site)
                    .with_context(|| format!("in {source}"))?
            }
            None => String::new(),
        };
        let page = render::page(&site, node, &body);
        let destination = generated.join(&node.route).join("index.html");
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::write(&destination, page)
            .with_context(|| format!("cannot write {}", destination.display()))?;
    }
    let render_elapsed = started.elapsed();
    let copy_started = Instant::now();
    let mut bytes = 0;
    for asset in &site.assets {
        let destination = generated.join(asset);
        fs::create_dir_all(destination.parent().unwrap())?;
        bytes += fs::copy(input.join(asset), destination)
            .with_context(|| format!("cannot copy {asset}"))?;
    }
    let copy_elapsed = copy_started.elapsed();
    fs::write(generated.join(".memoc"), MARKER)?;
    check_output(&output)?;
    let backup = staging.join("previous");
    let had_output = output.exists();
    if had_output {
        fs::rename(&output, &backup).context("cannot move previous output")?;
    }
    if let Err(error) = fs::rename(&generated, &output) {
        if had_output {
            fs::rename(&backup, &output)
                .context("cannot restore previous output; it remains in _memoc-build/previous")?;
        }
        return Err(error).context("cannot publish generated site");
    }
    cleanup.published = true;
    println!(
        "Built {} pages and {} assets ({} bytes) in {:.3}s: render {:.3}s, copy {:.3}s\n{}",
        site.nodes.len(),
        site.assets.len(),
        bytes,
        started.elapsed().as_secs_f64(),
        render_elapsed.as_secs_f64(),
        copy_elapsed.as_secs_f64(),
        output.display()
    );
    Ok(())
}

fn check_output(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(meta) => anyhow::ensure!(
            meta.is_dir() && !meta.is_symlink(),
            "_site must be a real directory"
        ),
    }
    if fs::read_dir(path)?.next().is_none() {
        return Ok(());
    }
    let marker = path.join(".memoc");
    let owned = fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && !m.is_symlink())
        && fs::read_to_string(marker).is_ok_and(|text| text == MARKER);
    anyhow::ensure!(
        owned,
        "refusing to replace nonempty _site not owned by memoc"
    );
    Ok(())
}

struct Cleanup {
    path: std::path::PathBuf,
    published: bool,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        // Keep a backup if publication AND rollback failed.
        if !self.path.join("previous").exists() || self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
