use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "memoc-test-{}-{unique}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, content: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_memoc"))
            .arg(&self.0)
            .output()
            .unwrap()
    }
    fn build(&self) {
        let output = self.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn html(&self, path: &str) -> String {
        fs::read_to_string(self.0.join("_site").join(path)).unwrap()
    }
    fn error(&self) -> String {
        let output = self.run();
        assert!(!output.status.success());
        String::from_utf8(output.stderr).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main_content(html: &str) -> &str {
    html.split("<main ").nth(1).unwrap()
}

#[test]
fn folders_work_without_index_files() {
    let f = Fixture::new();
    f.write("manuals/oven.md", "# Oven\n\nInstructions.");
    fs::create_dir(f.0.join("empty")).unwrap();
    f.build();
    assert!(main_content(&f.html("index.html")).contains("href=\"manuals/\""));
    assert!(main_content(&f.html("manuals/index.html")).contains("href=\"oven/\">Oven"));
    assert!(f.html("manuals/oven/index.html").contains("Instructions."));
    assert!(
        f.html("empty/index.html")
            .contains("<h2>Contents</h2><ul>\n</ul>")
    );
}

#[test]
fn readme_precedes_children_and_is_not_a_child() {
    let f = Fixture::new();
    f.write("README.md", "# My home\n\nAn introduction.");
    f.write("manuals/README.md", "# Appliance manuals\n\nStart here.");
    f.write("manuals/oven.md", "# Oven");
    f.build();
    let html = f.html("index.html");
    let body = main_content(&html);
    assert!(body.find("An introduction.").unwrap() < body.find("class=\"children\"").unwrap());
    assert!(body.contains("href=\"manuals/\">Appliance manuals"));
    assert!(!html.contains("README/"));
    assert!(!f.0.join("_site/README").exists());
    let html = f.html("manuals/index.html");
    assert!(html.find("Start here.").unwrap() < html.find("class=\"children\"").unwrap());
    assert!(html.contains("aria-current=\"page\">Appliance manuals"));
    assert!(!html.contains("href=\"/\""));
}

#[test]
fn index_is_an_ordinary_page_and_front_matter_is_markdown() {
    let f = Fixture::new();
    f.write("index.md", "---\nparent: Whatever\n---\n\n# Ordinary");
    f.build();
    assert!(f.html("index/index.html").contains("parent: Whatever"));
    assert!(main_content(&f.html("index.html")).contains("href=\"index/\">Ordinary"));
}

#[test]
fn rewrite_links_and_preserve_assets() {
    let f = Fixture::new();
    f.write("README.md", "# Home");
    f.write("manuals/README.md", "# Manuals");
    f.write("manuals/oven.md", "# Oven\n[Home](../README.md)\n[Parent](README.md)\n[Self](oven.md#oven)\n[Other](freezer.md?q=yes#cold)\n![Photo](photo.png)\n[PDF](/manuals/manual.pdf)\n[Web](https://example.com)\n[Email](mailto:a@example.com)");
    f.write("manuals/freezer.md", "# Freezer\n## Cold");
    f.write("manuals/photo.png", [0, 1, 2, 255]);
    f.write("manuals/manual.pdf", b"%PDF-1.4");
    f.build();
    let html = f.html("manuals/oven/index.html");
    for link in [
        "../../",
        "../",
        "./#oven",
        "../freezer/?q=yes#cold",
        "../manual.pdf",
        "https://example.com",
        "mailto:a@example.com",
    ] {
        assert!(
            html.contains(&format!("href=\"{link}\"")),
            "missing {link}: {html}"
        );
    }
    assert!(html.contains("src=\"../photo.png\""));
    assert_eq!(
        fs::read(f.0.join("_site/manuals/photo.png")).unwrap(),
        [0, 1, 2, 255]
    );
}

#[test]
fn unicode_paths_titles_and_duplicate_headings() {
    let f = Fixture::new();
    f.write(
        "README.md",
        "# <em>Home</em> & notes\n[Résumé](<résumé space.md>)",
    );
    f.write(
        "résumé space.md",
        "# Café & `tea`\n## Again\n## Again\n## Again-1\n## Main",
    );
    f.build();
    assert!(
        f.html("index.html")
            .contains("href=\"r%C3%A9sum%C3%A9%20space/\"")
    );
    let html = f.html("résumé space/index.html");
    for id in ["café-tea", "again", "again-1", "again-1-1", "main-1"] {
        assert!(html.contains(&format!("id=\"{id}\"")));
    }
    assert!(html.contains("<title>Café &amp; tea · Home &amp; notes</title>"));
}

#[test]
fn titles_fallback_and_children_sort() {
    let f = Fixture::new();
    f.write("zebra.md", "# Alpha");
    f.write("apple.md", "# Zebra");
    f.write("middle-page.md", "Some text, no heading.");
    f.build();
    let html = f.html("index.html");
    let body = main_content(&html);
    assert!(body.find(">Alpha</a>").unwrap() < body.find(">middle page</a>").unwrap());
    assert!(body.find(">middle page</a>").unwrap() < body.find(">Zebra</a>").unwrap());
    assert!(
        f.html("middle-page/index.html")
            .contains("<h1>middle page</h1>")
    );
}

#[test]
fn route_and_asset_collisions_fail_with_both_sources() {
    let f = Fixture::new();
    f.write("oven.md", "# Oven");
    f.write("oven/README.md", "# Other oven");
    let error = f.error();
    assert!(error.contains("oven.md") && error.contains("oven/README.md"));
    fs::remove_file(f.0.join("oven.md")).unwrap();
    f.write("oven/index.html", "do not overwrite");
    assert!(f.error().contains("output collision"));
}

#[test]
fn copied_file_cannot_be_output_directory() {
    let f = Fixture::new();
    f.write("oven.md", "# Oven");
    f.write("oven", "attachment");
    assert!(f.error().contains("output collision"));
}

#[test]
fn broken_links_fail_and_preserve_previous_build() {
    let f = Fixture::new();
    f.write("README.md", "# Original");
    f.build();
    let previous = f.html("index.html");
    f.write("README.md", "# Changed\n[Missing](absent.md)");
    let error = f.error();
    assert!(error.contains("README.md") && error.contains("absent.md"));
    assert_eq!(f.html("index.html"), previous);
    assert!(!f.0.join("_memoc-build").exists());
}

#[test]
fn stale_files_disappear_and_repeat_builds_are_identical() {
    let f = Fixture::new();
    f.write("old.md", "# Old");
    f.build();
    let original = f.html("index.html");
    f.build();
    assert_eq!(f.html("index.html"), original);
    fs::remove_file(f.0.join("old.md")).unwrap();
    f.build();
    assert!(!f.0.join("_site/old").exists());
}

#[test]
fn foreign_output_and_existing_build_directory_are_protected() {
    let f = Fixture::new();
    f.write("_site/keep.txt", "mine");
    assert!(f.error().contains("not owned"));
    assert_eq!(f.html("keep.txt"), "mine");
    fs::remove_dir_all(f.0.join("_site")).unwrap();
    f.write("_memoc-build/keep.txt", "pending");
    assert!(f.error().contains("another build"));
    assert!(f.0.join("_memoc-build/keep.txt").exists());
}

#[test]
fn hidden_entries_are_ignored_and_invalid_utf8_reports_source() {
    let f = Fixture::new();
    f.write(".private/secret.md", "# Secret");
    f.write("_draft.md", "# Draft");
    f.build();
    assert!(!f.html("index.html").contains("Secret"));
    assert!(!f.0.join("_site/_draft").exists());
    f.write("bad.md", [255]);
    assert!(f.error().contains("bad.md"));
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_and_output_symlink_is_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.write("secret.md", "private");
    symlink(outside.0.join("secret.md"), f.0.join("secret.md")).unwrap();
    symlink(&outside.0, f.0.join("external")).unwrap();
    f.build();
    assert!(!f.0.join("_site/secret").exists());
    assert!(!f.0.join("_site/external").exists());
    fs::remove_dir_all(f.0.join("_site")).unwrap();
    symlink(&outside.0, f.0.join("_site")).unwrap();
    assert!(f.error().contains("real directory"));
    assert_eq!(
        fs::read_to_string(outside.0.join("secret.md")).unwrap(),
        "private"
    );
}

#[test]
fn links_cannot_escape_source_root() {
    let f = Fixture::new();
    f.write("README.md", "[Outside](%2e%2e/outside.md)");
    assert!(f.error().contains("escapes input folder"));
}

#[test]
fn markdown_extensions_and_raw_html_work() {
    let f = Fixture::new();
    f.write("README.md", "# Home\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n- [x] Done\n\n~~Gone~~\n\n<strong>HTML</strong>\n\n```rust\nlet x = 1;\n```");
    f.build();
    let html = f.html("index.html");
    for expected in [
        "<table>",
        "type=\"checkbox\"",
        "<del>Gone</del>",
        "<strong>HTML</strong>",
        "language-rust",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn explicit_build_supports_a_folder_or_the_current_directory() {
    let input = Fixture::new();
    input.write("README.md", "# Explicit build");
    let output = Command::new(env!("CARGO_BIN_EXE_memoc"))
        .arg("build")
        .arg(&input.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(input.html("index.html").contains("Explicit build"));
    input.write("README.md", "# Current directory");
    let output = Command::new(env!("CARGO_BIN_EXE_memoc"))
        .arg("build")
        .current_dir(&input.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(input.html("index.html").contains("Current directory"));
}

#[test]
fn command_line_is_minimal() {
    let bin = Path::new(env!("CARGO_BIN_EXE_memoc"));
    assert!(
        Command::new(bin)
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        Command::new(bin)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("memoc watch [folder]")
    );
    for args in [
        vec![],
        vec!["--theme"],
        vec!["a", "b"],
        vec!["watch", "--unknown"],
        vec!["watch", "a", "b"],
        vec!["build", "--unknown"],
        vec!["build", "a", "b"],
    ] {
        assert!(
            !Command::new(bin)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let input = Fixture::new();
    let output = Command::new(bin)
        .arg("watch")
        .arg(input.0.join("missing"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot open")
    );
}
