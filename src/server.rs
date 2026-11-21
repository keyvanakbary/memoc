use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener},
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};

pub fn run(root: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot open site directory {}", root.display()))?;
    anyhow::ensure!(root.is_dir(), "{} is not a directory", root.display());
    let listener = bind()?;
    let address = listener.local_addr()?;
    println!("Serving {} at http://{address}/", root.display());
    serve(listener, root, Arc::new(RwLock::new(())))?;
    Ok(())
}

pub fn bind() -> Result<TcpListener> {
    let address = listen_address(std::env::var("MEMOC_BIND").ok().as_deref())?;
    TcpListener::bind(address)
        .with_context(|| format!("cannot serve at {address}; check that the port is available"))
}

fn listen_address(value: Option<&str>) -> Result<SocketAddr> {
    value
        .unwrap_or("127.0.0.1:4000")
        .parse()
        .context("MEMOC_BIND must be an IP address and port, for example 0.0.0.0:4000")
}

pub fn serve(listener: TcpListener, root: PathBuf, gate: Arc<RwLock<()>>) -> io::Result<()> {
    let root = Arc::new(root);
    for connection in listener.incoming() {
        let mut stream = connection?;
        // Each active connection holds one reference. Only this accept loop adds them.
        if Arc::strong_count(&root) > 32 {
            stream.set_write_timeout(Some(Duration::from_secs(1)))?;
            let _ = status(&mut stream, "503 Service Unavailable", false, "");
            continue;
        }
        let root = Arc::clone(&root);
        let gate = Arc::clone(&gate);
        thread::spawn(move || -> io::Result<()> {
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            respond(&mut reader, &mut stream, &root, &gate)
        });
    }
    Ok(())
}

// Read/write streams keep the HTTP handler testable without opening a socket.
fn respond(
    input: &mut impl BufRead,
    output: &mut impl Write,
    root: &Path,
    gate: &RwLock<()>,
) -> io::Result<()> {
    const MAX_HEADER: usize = 8192;
    let mut header = Vec::new();
    loop {
        let count = (&mut *input)
            .take((MAX_HEADER + 1 - header.len()) as u64)
            .read_until(b'\n', &mut header)?;
        if header.len() > MAX_HEADER {
            return status(output, "431 Request Header Fields Too Large", false, "");
        }
        if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") {
            break;
        }
        if count == 0 {
            return status(output, "400 Bad Request", false, "");
        }
    }
    let Ok(header) = std::str::from_utf8(&header) else {
        return status(output, "400 Bad Request", false, "");
    };
    let parts: Vec<_> = header
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let [method, target, version] = parts.as_slice() else {
        return status(output, "400 Bad Request", false, "");
    };
    let head = *method == "HEAD";
    if !matches!(*version, "HTTP/1.0" | "HTTP/1.1")
        || !target.starts_with('/')
        || target.starts_with("//")
    {
        return status(output, "400 Bad Request", head, "");
    }
    if target.split('?').next() == Some("/mcp") {
        return mcp_response(input, output, method, header, root, gate);
    }
    if !matches!(*method, "GET" | "HEAD") {
        return status(
            output,
            "405 Method Not Allowed",
            false,
            "Allow: GET, HEAD\r\n",
        );
    }
    let (url_path, query) = target
        .split_once('?')
        .map_or((*target, ""), |(path, _)| (path, &target[path.len()..]));
    let Some(decoded) = decode_path(url_path) else {
        return status(output, "400 Bad Request", head, "");
    };
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || decoded.split('/').any(|part| part.starts_with('.'))
    {
        return status(output, "404 Not Found", head, "");
    }
    let _guard = gate.read().unwrap();
    let Ok(root) = root.canonicalize() else {
        return status(output, "503 Service Unavailable", head, "");
    };
    let Ok(mut path) = root.join(relative).canonicalize() else {
        return status(output, "404 Not Found", head, "");
    };
    if !path.starts_with(&root) {
        return status(output, "404 Not Found", head, "");
    }
    if path.is_dir() {
        if !url_path.ends_with('/') {
            return status(
                output,
                "301 Moved Permanently",
                head,
                &format!("Location: {url_path}/{query}\r\n"),
            );
        }
        // Canonicalize again so even a replaced index symlink cannot expose another directory.
        let Ok(index) = path.join("index.html").canonicalize() else {
            return status(output, "404 Not Found", head, "");
        };
        if !index.starts_with(&root) {
            return status(output, "404 Not Found", head, "");
        }
        path = index;
    }
    if path
        .strip_prefix(&root)
        .unwrap()
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return status(output, "404 Not Found", head, "");
    }
    let Ok(mut file) = File::open(&path) else {
        return status(output, "404 Not Found", head, "");
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return status(output, "404 Not Found", head, "");
    }
    headers(output, "200 OK", content_type(&path), metadata.len(), "")?;
    if !head {
        io::copy(&mut file, output)?;
    }
    Ok(())
}

fn mcp_response(
    input: &mut impl BufRead,
    output: &mut impl Write,
    method: &str,
    header: &str,
    root: &Path,
    gate: &RwLock<()>,
) -> io::Result<()> {
    let mut fields = HashMap::new();
    for line in header.lines().skip(1).filter(|s| !s.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return status(output, "400 Bad Request", false, "");
        };
        if fields
            .insert(name.to_ascii_lowercase(), value.trim())
            .is_some()
        {
            return status(output, "400 Bad Request", false, "");
        }
    }
    if let Some(origin) = fields.get("origin") {
        let allowed = std::env::var("MEMOC_ORIGIN").ok();
        // Native clients omit Origin. Browsers must use the configured origin or loopback.
        let local = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .is_some_and(|authority| {
                fields.get("host") == Some(&authority)
                    && ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
                        authority == *host
                            || authority
                                .strip_prefix(host)
                                .and_then(|s| s.strip_prefix(':'))
                                .is_some_and(|port| port.parse::<u16>().is_ok())
                    })
            });
        if !local && allowed.as_deref() != Some(*origin) {
            return status(output, "403 Forbidden", method == "HEAD", "");
        }
    }
    if method != "POST" {
        return status(
            output,
            "405 Method Not Allowed",
            method == "HEAD",
            "Allow: POST\r\n",
        );
    }
    if !fields.get("content-type").is_some_and(|v| {
        v.split(';')
            .next()
            .unwrap()
            .trim()
            .eq_ignore_ascii_case("application/json")
    }) {
        return status(output, "415 Unsupported Media Type", false, "");
    }
    let bytes = match read_body(input, &fields) {
        Ok(bytes) => bytes,
        Err(code) => return status(output, code, false, ""),
    };
    let reply = {
        let _guard = gate.read().unwrap();
        crate::mcp::dispatch(root, &fields, &bytes)
    };
    let body = reply.body.map(|v| v.to_string()).unwrap_or_default();
    headers(
        output,
        reply.status,
        "application/json",
        body.len() as u64,
        "",
    )?;
    output.write_all(body.as_bytes())
}

fn read_body(
    input: &mut impl BufRead,
    fields: &HashMap<String, &str>,
) -> Result<Vec<u8>, &'static str> {
    const MAX_BODY: usize = 64 * 1024;
    const BAD: &str = "400 Bad Request";
    const LARGE: &str = "413 Content Too Large";
    let mut body = Vec::new();
    if let Some(encoding) = fields.get("transfer-encoding") {
        if fields.contains_key("content-length") || !encoding.eq_ignore_ascii_case("chunked") {
            return Err(BAD);
        }
        // Bound chunk framing as well as decoded data, even for empty or very small chunks.
        let mut bounded = input.take((2 * MAX_BODY) as u64);
        let mut line = String::new();
        loop {
            line.clear();
            bounded.read_line(&mut line).map_err(|_| BAD)?;
            let line = line.strip_suffix("\r\n").ok_or(BAD)?;
            let digits = line.split(';').next().unwrap();
            if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(BAD);
            }
            let count = usize::from_str_radix(digits, 16).map_err(|_| BAD)?;
            if count == 0 {
                break;
            }
            if count > MAX_BODY - body.len() {
                return Err(LARGE);
            }
            let start = body.len();
            body.resize(start + count, 0);
            bounded.read_exact(&mut body[start..]).map_err(|_| BAD)?;
            let mut end = [0; 2];
            bounded.read_exact(&mut end).map_err(|_| BAD)?;
            if end != *b"\r\n" {
                return Err(BAD);
            }
        }
        // Trailers carry no MCP metadata; consume them to finish the message.
        loop {
            line.clear();
            bounded.read_line(&mut line).map_err(|_| BAD)?;
            if line == "\r\n" {
                break;
            }
            if !line.ends_with("\r\n") || !line.contains(':') {
                return Err(BAD);
            }
        }
    } else {
        let length = fields.get("content-length").ok_or("411 Length Required")?;
        if !length.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BAD);
        }
        let length = length.parse::<usize>().map_err(|_| BAD)?;
        if length > MAX_BODY {
            return Err(LARGE);
        }
        body.resize(length, 0);
        input.read_exact(&mut body).map_err(|_| BAD)?;
    }
    Ok(body)
}

fn headers(
    output: &mut impl Write,
    status: &str,
    content_type: &str,
    length: u64,
    extra: &str,
) -> io::Result<()> {
    write!(
        output,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n{extra}\r\n"
    )
}

fn status(output: &mut impl Write, code: &str, head: bool, extra: &str) -> io::Result<()> {
    let body = format!("{code}\n");
    headers(
        output,
        code,
        "text/plain; charset=utf-8",
        body.len() as u64,
        extra,
    )?;
    if !head {
        output.write_all(body.as_bytes())?;
    }
    Ok(())
}

fn decode_path(path: &str) -> Option<String> {
    let mut input = path.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = input.next() {
        let byte = if byte == b'%' {
            let a = (input.next()? as char).to_digit(16)?;
            let b = (input.next()? as char).to_digit(16)?;
            (a * 16 + b) as u8
        } else {
            byte
        };
        if byte.is_ascii_control() || byte == b'\\' {
            return None;
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded).ok()
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Folder;
    use std::fs;

    #[test]
    fn local_runs_use_loopback_and_containers_can_bind_all_interfaces() {
        assert_eq!(listen_address(None).unwrap().to_string(), "127.0.0.1:4000");
        assert_eq!(
            listen_address(Some("0.0.0.0:4000")).unwrap().to_string(),
            "0.0.0.0:4000"
        );
        assert!(listen_address(Some("invalid")).is_err());
    }

    fn request(root: &Path, request: &str) -> Vec<u8> {
        let mut response = Vec::new();
        respond(
            &mut io::Cursor::new(request),
            &mut response,
            root,
            &RwLock::new(()),
        )
        .unwrap();
        response
    }

    #[test]
    fn serves_pages_assets_and_unicode_paths_with_correct_headers() {
        let root = Folder::new();
        root.write("index.html", "<h1>Home</h1>");
        root.write("nested/index.html", "<h1>Nested</h1>");
        root.write("résumé space/index.html", "Unicode");
        root.write("_memoc.css", "body{}");
        root.write("manual.pdf", [0, 255, 1]);
        for (url, mime, body) in [
            ("/", "text/html; charset=utf-8", b"<h1>Home</h1>".as_slice()),
            (
                "/nested/?q=yes",
                "text/html; charset=utf-8",
                b"<h1>Nested</h1>",
            ),
            (
                "/r%C3%A9sum%C3%A9%20space/",
                "text/html; charset=utf-8",
                b"Unicode",
            ),
            ("/_memoc.css", "text/css; charset=utf-8", b"body{}"),
            ("/manual.pdf", "application/pdf", &[0, 255, 1]),
        ] {
            let response = request(
                &root.0,
                &format!("GET {url} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            );
            let text = String::from_utf8_lossy(&response);
            assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(text.contains(&format!("Content-Type: {mime}\r\n")));
            assert!(text.contains(&format!("Content-Length: {}\r\n", body.len())));
            assert!(text.contains("Cache-Control: no-store\r\n"));
            assert!(response.ends_with(body));
        }
    }

    #[test]
    fn head_redirect_missing_page_and_method_handling() {
        let root = Folder::new();
        root.write("folder/index.html", "Hello");
        let head = request(&root.0, "HEAD /folder/ HTTP/1.1\r\n\r\n");
        let head = String::from_utf8(head).unwrap();
        assert!(head.contains("Content-Length: 5\r\n"));
        assert!(head.ends_with("\r\n\r\n"));
        let redirect =
            String::from_utf8(request(&root.0, "GET /folder?q=yes HTTP/1.1\r\n\r\n")).unwrap();
        assert!(redirect.starts_with("HTTP/1.1 301"));
        assert!(redirect.contains("Location: /folder/?q=yes\r\n"));
        for (method, path, code) in [
            ("GET", "/missing", "404"),
            ("HEAD", "/missing", "404"),
            ("POST", "/folder/", "405"),
        ] {
            let response = String::from_utf8(request(
                &root.0,
                &format!("{method} {path} HTTP/1.1\r\n\r\n"),
            ))
            .unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {code}")));
            if method == "HEAD" {
                assert!(response.ends_with("\r\n\r\n"));
            }
        }
        assert!(
            String::from_utf8(request(&root.0.join("not-built"), "GET / HTTP/1.1\r\n\r\n"))
                .unwrap()
                .starts_with("HTTP/1.1 503")
        );
    }

    #[test]
    fn rejects_traversal_hidden_files_bad_encoding_and_oversized_headers() {
        let root = Folder::new();
        root.write(".memoc", "private");
        for url in [
            "/../secret",
            "/%2e%2e/secret",
            "/nested/%2e%2e/secret",
            "/.memoc",
            "/%2ememoc",
        ] {
            let response = request(&root.0, &format!("GET {url} HTTP/1.1\r\n\r\n"));
            assert!(response.starts_with(b"HTTP/1.1 404"), "{url}");
        }
        for url in [
            "/%",
            "/%GG",
            "/%00",
            "/%5c..",
            "//outside",
            "/%0d%0aInjected",
        ] {
            assert!(
                request(&root.0, &format!("GET {url} HTTP/1.1\r\n\r\n"))
                    .starts_with(b"HTTP/1.1 400"),
                "{url}"
            );
        }
        assert!(request(&root.0, "garbage\r\n\r\n").starts_with(b"HTTP/1.1 400"));
        assert!(
            request(
                &root.0,
                &format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(8192))
            )
            .starts_with(b"HTTP/1.1 431")
        );
    }

    #[test]
    fn stateless_mcp_discovery_search_and_fetch() {
        use serde_json::{Value, json};
        let input = Folder::new();
        input.write("mcp.md", "# Help\n\nAn orange dial");
        crate::build(&input.0).unwrap();
        let root = input.0.join("_site");
        let rpc = |body: Value, headers: &str| {
            let raw = body.to_string();
            let message = format!(
                "POST /mcp HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}\r\n{raw}",
                raw.len()
            );
            let response = String::from_utf8(request(&root, &message)).unwrap();
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            (
                headers.to_owned(),
                serde_json::from_str::<Value>(body).unwrap_or(Value::Null),
            )
        };
        let legacy = "MCP-Protocol-Version: 2025-11-25\r\n";
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
        assert_eq!(
            rpc(init, "").1["result"]["protocolVersion"],
            crate::mcp::LEGACY
        );
        assert!(
            rpc(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                legacy
            )
            .0
            .starts_with("HTTP/1.1 202")
        );
        let discover = json!({"jsonrpc":"2.0","id":2,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}});
        let modern = "MCP-Protocol-Version: 2026-07-28\r\nMcp-Method: server/discover\r\n";
        assert_eq!(
            rpc(discover.clone(), modern).1["result"]["supportedVersions"][0],
            crate::mcp::CURRENT
        );
        assert_eq!(rpc(discover, legacy).1["error"]["code"], -32020);
        let call = |name: &str, args| json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}});
        let search = call("search", json!({"query":"orange"}));
        let (_, result) = rpc(search.clone(), legacy);
        let hit = &result["result"]["structuredContent"]["passages"][0];
        assert_eq!(hit["url"], "/mcp/#help");
        let (_, result) = rpc(call("fetch", json!({"id":hit["id"]})), legacy);
        assert_eq!(
            result["result"]["structuredContent"]["passages"][0]["text"],
            "An orange dial"
        );
        assert!(
            rpc(search.clone(), "Origin: https://other.example\r\n")
                .0
                .starts_with("HTTP/1.1 403")
        );
        assert_eq!(
            rpc(call("search", json!({"query":"a","limit":21})), legacy).1["result"]["isError"],
            true
        );
        assert!(request(&root, "GET /mcp/ HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 200"));
        assert!(request(&root, "GET /mcp HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 405"));
        assert!(
            request(&root, "GET /.memoc-search.sqlite3 HTTP/1.1\r\n\r\n")
                .starts_with(b"HTTP/1.1 404")
        );
        fs::remove_file(root.join(".memoc-search.sqlite3")).unwrap();
        assert!(
            rpc(search, legacy).1["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("memoc build")
        );
        assert!(request(&root, "GET / HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 200"));
    }

    #[test]
    fn mcp_body_framing_and_limits() {
        let root = Folder::new();
        let message = |fields: &str, body: &str| {
            request(
                &root.0,
                &format!(
                    "POST /mcp HTTP/1.1\r\nContent-Type: application/json\r\n{fields}\r\n{body}"
                ),
            )
        };
        let rpc = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let chunks = format!("{:x}\r\n{rpc}\r\n0\r\n\r\n", rpc.len());
        assert!(message("Transfer-Encoding: chunked\r\n", &chunks).starts_with(b"HTTP/1.1 200"));
        assert!(
            message(
                "Transfer-Encoding: chunked\r\nContent-Length: 1\r\n",
                &chunks
            )
            .starts_with(b"HTTP/1.1 400")
        );
        assert!(message("Content-Length: 65537\r\n", "").starts_with(b"HTTP/1.1 413"));
        assert!(
            message("Transfer-Encoding: chunked\r\n", "10001\r\n").starts_with(b"HTTP/1.1 413")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cannot_serve_symlinks_outside_output() {
        let root = Folder::new();
        let outside = Folder::new();
        outside.write("secret", "private");
        std::os::unix::fs::symlink(outside.0.join("secret"), root.0.join("file")).unwrap();
        fs::create_dir(root.0.join("page")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret"), root.0.join("page/index.html"))
            .unwrap();
        for url in ["/file", "/page/"] {
            assert!(
                request(&root.0, &format!("GET {url} HTTP/1.1\r\n\r\n"))
                    .starts_with(b"HTTP/1.1 404")
            );
        }
    }
}
