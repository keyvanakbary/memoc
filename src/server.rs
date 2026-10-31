use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};

pub fn serve(listener: TcpListener, root: PathBuf, gate: Arc<RwLock<()>>) {
    let root = Arc::new(root);
    for connection in listener.incoming() {
        let Ok(mut stream) = connection else {
            eprintln!("memoc: preview server could not accept a connection");
            break;
        };
        let root = Arc::clone(&root);
        let gate = Arc::clone(&gate);
        thread::spawn(move || -> io::Result<()> {
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            respond(&mut reader, &mut stream, &root, &gate)
        });
    }
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
