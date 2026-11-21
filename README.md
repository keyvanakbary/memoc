# Memoc

A small, fast static site builder for plain Markdown. One folder in, a static website out. No configuration or front matter.

```sh
memoc build --serve ./docs
```

This builds the site once into `docs/_site/` and serves its HTML, stylesheet, and assets at **http://127.0.0.1:4000**. Press Ctrl+C to stop the server.

For a separate build and deployment, run `memoc build ./docs`, then `memoc serve ./docs/_site`. Only the generated directory and the Memoc binary are needed on the serving host. With no directory argument, `memoc serve` serves `./_site`.

During editing, use `memoc watch ./docs` to rebuild when files change. Refresh the browser to see updates.

The HTTP server is built into the Memoc binary using Rust’s standard library. To expose the site on all network interfaces, set the listening address and port when starting Memoc:

```sh
MEMOC_BIND=0.0.0.0:4000 memoc serve ./docs/_site
```

## Query the site over MCP

Connect a Streamable HTTP MCP client to `http://127.0.0.1:4000/mcp`. The endpoint is read-only and unauthenticated. Anyone who can reach it can query the published content. Each request is independent, so a passage ID returned by one client also works in another.

| Tool | Arguments | Result |
| --- | --- | --- |
| `search` | `query`, optional `limit` (default 5, maximum 20) | BM25-ranked excerpts, passage IDs, and site-relative citation URLs |
| `fetch` | `id`, optional `context` (default 1, range 0–2) | The full passage and that many neighbors on either side in the same document, in reading order |

For example, call `search` with `{"query":"oven control lock"}`, then pass a result’s `id` to `fetch` to read the surrounding instructions. Each result links to the Markdown heading or physical PDF page. A PDF needs embedded text to be searchable. Image-only pages are skipped, and a PDF that cannot be indexed still ships as a download.

Every build writes the index to `_site/.memoc-search.sqlite3`. Copy the entire directory, including hidden files, when deploying. Serving only needs the Memoc binary and read access to the output. Rebuild older sites once to enable search. A Markdown page named `mcp.md` is served at `/mcp/`. The exact path `/mcp` belongs to the protocol.

Native clients can connect directly. For a browser client hosted on the same public origin as Memoc, set `MEMOC_ORIGIN` to that exact origin (for example `https://docs.example.org`). Same-origin loopback requests work by default.

### Connect Claude Code or Codex CLI

Start Memoc with `memoc build --serve ./docs` and leave it running. In another terminal, run the commands for your client. For a deployed site, replace `http://127.0.0.1:4000/mcp` with its full MCP URL, such as `https://docs.example.org/mcp`.

**Claude Code** — Run these from the project where you want to use Memoc. The second command shows the connection status. [Claude Code MCP guide](https://code.claude.com/docs/en/mcp)

```sh
claude mcp add --transport http memoc http://127.0.0.1:4000/mcp
claude mcp get memoc
```

**Codex CLI** — Register Memoc and check the saved URL. [Codex MCP guide](https://developers.openai.com/codex/mcp)

```sh
codex mcp add memoc --url http://127.0.0.1:4000/mcp
codex mcp get memoc
```

Open a new session in your client and try a question about your documents, for example: “Use Memoc to search for oven cleaning instructions. Fetch the matching passage and cite the manual page.”

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

### Package a prebuilt site

With a Memoc image that includes `serve`, use two stages to package the generated files and the HTTP server together:

```dockerfile
FROM ghcr.io/keyvanakbary/memoc:latest AS build
COPY . /docs/
RUN ["/memoc", "build", "/docs"]

FROM ghcr.io/keyvanakbary/memoc:latest
COPY --from=build /docs/_site /site
CMD ["serve", "/site"]
```

The runtime stage listens on port 4000. Publish it with `-p 80:4000` to serve the site on host port 80.

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
