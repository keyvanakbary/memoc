# Memoc

A small, fast static site builder for plain Markdown. One folder in, a static website out. No configuration or front matter.

```sh
memoc build ./docs
memoc watch ./docs
```

`build` writes the site to `docs/_site/`. `watch` serves it at **http://127.0.0.1:4000** and rebuilds when files change. Refresh the browser to see updates. Both commands default to the current folder.

## Binaries

Download a standalone Linux binary from [GitHub Releases](https://github.com/keyvanakbary/memoc/releases/latest). Choose `amd64` for Intel/AMD or `arm64` for ARM. No Rust installation is needed.

For Linux AMD64:

```sh
curl -fLO https://github.com/keyvanakbary/memoc/releases/latest/download/memoc-linux-amd64.tar.gz
tar -xzf memoc-linux-amd64.tar.gz
./memoc build ./docs
./memoc watch ./docs
```

For ARM64, replace `amd64` with `arm64`. Each release includes `SHA256SUMS` for verifying downloads. Put `memoc` in a directory on your `PATH` to run it without `./`.

## Docker

Images are published to `ghcr.io/keyvanakbary/memoc` for AMD64 and ARM64. Mount a writable Markdown folder at `/docs` and choose a command explicitly:

```sh
docker run --rm --init --user "$(id -u):$(id -g)" \
  -v "$PWD/docs:/docs" \
  ghcr.io/keyvanakbary/memoc:latest build /docs

docker run --rm --init --user "$(id -u):$(id -g)" \
  -v "$PWD/docs:/docs" -p 127.0.0.1:4000:4000 \
  ghcr.io/keyvanakbary/memoc:latest watch /docs
```

The generated site appears in `./docs/_site/`. The `--user` option keeps generated files owned by your user. Use `latest` for the newest release or a full version tag such as `0.1.1` to pin a release.

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

The output is plain HTML and CSS, with no browser JavaScript.
