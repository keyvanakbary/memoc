# Memoc

A small, fast static site builder for plain Markdown. One folder in, a static website out. No configuration or front matter.

```sh
memoc build ./docs
memoc watch ./docs
```

`build` writes the site to `docs/_site/`. `watch` serves it at **http://127.0.0.1:4000** and rebuilds when files change. Refresh the browser to see updates. Both commands default to the current folder.

## Content

```text
docs/
  README.md          → /
  guide.md           → /guide/
  reference/
    README.md        → /reference/
    syntax.md        → /reference/syntax/
```

Folders define the hierarchy and navigation. Every folder gets a page listing its children; an optional `README.md` appears before that list. Titles come from the first H1 or the filename.

Images and downloads are copied unchanged. Relative Markdown links are rewritten to their generated URLs. Dot-prefixed and underscore-prefixed entries are ignored.

The output is plain HTML and CSS, with no browser JavaScript. Docker images support AMD64 and ARM64, mount content at `/docs`, and accept an explicit `build /docs` or `watch /docs` command.
