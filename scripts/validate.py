#!/usr/bin/env python3
"""Reproducible integration validation and benchmarks; Python standard library only."""
import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
from urllib.parse import unquote, urljoin, urlsplit


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []
        self.ids = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.append(attrs["id"])
        for key in ("href", "src"):
            if key in attrs:
                self.links.append(attrs[key])


def crawl(site):
    pages = list(site.rglob("*.html"))
    for page in pages:
        parser = Links()
        parser.feed(page.read_text())
        assert len(parser.ids) == len(set(parser.ids)), f"duplicate IDs: {page}"
        route = page.relative_to(site).as_posix().removesuffix("index.html")
        base = "https://example.test/prefix/" + route
        for link in parser.links:
            url = urlsplit(urljoin(base, link))
            if url.netloc != "example.test" or url.scheme not in ("http", "https"):
                continue
            assert url.path.startswith("/prefix/"), (page, link, "escaped hosting prefix")
            target = site / unquote(url.path.removeprefix("/prefix/"))
            if url.path.endswith("/"):
                target /= "index.html"
            assert target.is_file(), (page, link, target)
            if url.fragment and target.suffix == ".html":
                destination = Links()
                destination.feed(target.read_text())
                assert unquote(url.fragment) in destination.ids, (page, link, "missing anchor")
    return len(pages)


def measure(binary, source):
    report = source.parent / (source.name + "-resource.txt")
    output = subprocess.run(
        ["/usr/bin/time", "-f", "%e %M", "-o", str(report), str(binary), str(source)],
        capture_output=True, text=True, check=True,
    )
    seconds, rss = report.read_text().split()
    size = sum(path.stat().st_size for path in (source / "_site").rglob("*") if path.is_file())
    return {"elapsed_seconds": float(seconds), "peak_rss_kib": int(rss), "output_bytes": size,
            "builder": output.stdout.strip()}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def content_fixture(original, destination):
    expected_routes = {Path("index.html")}
    assets = []
    for source in original.rglob("*"):
        relative = source.relative_to(original)
        if any(part.startswith((".", "_")) for part in relative.parts):
            continue
        if source.is_symlink():
            continue
        if source.is_dir():
            (destination / relative).mkdir(parents=True, exist_ok=True)
            expected_routes.add(relative / "index.html")
            continue
        if not source.is_file():
            continue
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        expected_routes.update(parent / "index.html" for parent in relative.parents)
        if source.suffix == ".md":
            if source.name == "README.md":
                expected_routes.add(relative.parent / "index.html")
            else:
                expected_routes.add(relative.with_suffix("") / "index.html")
        else:
            assets.append((source, relative))
        shutil.copyfile(source, target)
    return expected_routes, assets


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docs", type=Path, help="Validate a copy of a Markdown folder")
    parser.add_argument("--binary", type=Path, default=Path("target/release/memoc"))
    parser.add_argument("--keep", type=Path, help="Keep fixtures and report in a new directory")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    temporary = None
    if args.keep:
        args.keep.mkdir(parents=True, exist_ok=False)
        work = args.keep.resolve()
    else:
        temporary = tempfile.TemporaryDirectory(prefix="memoc-validation-")
        work = Path(temporary.name)
    results = {"binary_bytes": binary.stat().st_size}
    if args.docs:
        source = work / "docs"
        source.mkdir()
        routes, assets = content_fixture(args.docs.resolve(strict=True), source)
        results["docs"] = measure(binary, source)
        actual = {path.relative_to(source / "_site") for path in (source / "_site").rglob("index.html")}
        assert actual == routes, (actual, routes)
        for original, relative in assets:
            assert digest(original) == digest(source / "_site" / relative), relative
        results["docs"]["verified_pages"] = crawl(source / "_site")
        results["docs"]["verified_assets"] = len(assets)
    source = work / "large"
    source.mkdir()
    for group in range(100):
        folder = source / f"group-{group:03}"
        folder.mkdir()
        for number in range(100):
            (folder / f"page-{number:03}.md").write_text(
                f"# Page {number:03}\n\nExample documentation in group {group}.\n\n"
                f"[Next](page-{(number + 1) % 100:03}.md#details)\n\n"
                "## Details\n\n- First item\n- Second item\n\n```rust\nlet answer = 42;\n```\n"
            )
    results["10000_markdown_files"] = measure(binary, source)
    assert sum(1 for _ in (source / "_site").rglob("*.html")) == 10101
    # Crawl the small real site fully; sample the large corpus to keep validation cheap.
    page = source / "_site/group-000/page-000/index.html"
    assert 'href="../page-001/#details"' in page.read_text()
    print(json.dumps(results, indent=2))
    if args.keep:
        (work / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"Fixtures: {work}")
    if temporary:
        temporary.cleanup()


if __name__ == "__main__":
    main()
