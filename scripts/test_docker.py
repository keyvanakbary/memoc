#!/usr/bin/env python3
"""Smoke-test a built image using a mounted folder and a published loopback port."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True, stderr=subprocess.STDOUT).strip()


def wait_for(check):
    deadline = time.monotonic() + 30
    last_error = None
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (HTTPError, URLError, OSError) as error:
            last_error = error
        time.sleep(0.1)
    raise AssertionError(f"Container did not reach the expected state: {last_error}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="memoc:local")
    args = parser.parse_args()
    no_command = subprocess.run(["docker", "run", "--rm", args.image], capture_output=True, text=True)
    assert no_command.returncode != 0 and "usage:" in no_command.stderr, no_command
    with tempfile.TemporaryDirectory(prefix="memoc-docker-") as temporary:
        source = Path(temporary)
        (source / "README.md").write_text("# Docker preview\n\nMounted content.")
        (source / "manuals").mkdir()
        (source / "manuals/oven.md").write_text("# Oven\n\n[Manual](manual.pdf)")
        pdf = b"%PDF-1.4\nbyte-identical attachment\n"
        (source / "manuals/manual.pdf").write_bytes(pdf)
        docker(
            "run", "--rm", "--init", "--user", f"{os.getuid()}:{os.getgid()}",
            "--volume", f"{source}:/docs", args.image, "build", "/docs",
        )
        assert "Mounted content." in (source / "_site/index.html").read_text()
        assert (source / "_site/manuals/manual.pdf").read_bytes() == pdf
        container = docker(
            "run", "--detach", "--init", "--user", f"{os.getuid()}:{os.getgid()}",
            "--publish", "127.0.0.1::4000", "--volume", f"{source}:/docs", args.image, "watch", "/docs",
        )
        try:
            address = docker("port", container, "4000/tcp").splitlines()[0]
            base = f"http://{address}"

            def get(path="/"):
                with urlopen(base + path, timeout=2) as response:
                    assert response.headers["Cache-Control"] == "no-store"
                    return response.read()

            wait_for(lambda: b"Mounted content." in get())
            assert b"Oven" in get("/manuals/oven/")
            assert get("/manuals/manual.pdf") == pdf
            with urlopen(Request(base + "/", method="HEAD"), timeout=2) as response:
                assert response.read() == b""
                assert int(response.headers["Content-Length"]) > 0

            (source / "README.md").write_text("# Updated preview\n\nSaved change.")
            wait_for(lambda: b"Saved change." in get())
            previous = get()
            (source / "README.md").write_text("# Invalid edit\n[Missing](missing.md)")
            wait_for(lambda: "Build failed" in docker("logs", container))
            assert get() == previous
            (source / "README.md").write_text("# Recovered preview")
            wait_for(lambda: b"Recovered preview" in get())

            (source / "new.md").write_text("# Added page")
            wait_for(lambda: b"Added page" in get("/new/"))
            (source / "new.md").unlink()

            def deleted():
                try:
                    get("/new/")
                except HTTPError as error:
                    return error.code == 404
                return False

            wait_for(deleted)
            assert (source / "_site/index.html").stat().st_uid == os.getuid()
            print("Docker smoke test passed: explicit build and watch commands, /docs mount, published port, assets, HEAD, rebuilds, error recovery, and deletion.")
        except Exception:
            subprocess.run(["docker", "logs", container], check=False)
            raise
        finally:
            subprocess.run(["docker", "rm", "--force", container], check=False, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
