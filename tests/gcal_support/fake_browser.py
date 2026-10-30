#!/usr/bin/env python3
"""Fake browser for bob gcal login tests. Hits the loopback callback."""
from __future__ import annotations

import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


def main() -> int:
    if len(sys.argv) < 2:
        sys.stderr.write("fake browser: missing URL\n")
        return 2
    auth_url = sys.argv[1]
    parsed = urllib.parse.urlparse(auth_url)
    query = urllib.parse.parse_qs(parsed.query)
    redirect = (query.get("redirect_uri") or [None])[0]
    state = (query.get("state") or [None])[0]
    if not redirect or not state:
        sys.stderr.write("fake browser: auth URL missing redirect_uri or state\n")
        return 2
    error = os.environ.get("BOB_GCAL_FAKE_BROWSER_ERROR")
    deny_once = os.environ.get("BOB_GCAL_FAKE_BROWSER_DENY_ONCE")
    if deny_once:
        marker = Path(deny_once)
        if not marker.exists():
            marker.parent.mkdir(parents=True, exist_ok=True)
            marker.write_text("1", encoding="utf-8")
            error = "access_denied"
    code = os.environ.get("BOB_GCAL_FAKE_BROWSER_CODE", "fake/code+1")
    if error:
        callback = (
            f"{redirect}?state={urllib.parse.quote(state, safe='')}"
            f"&error={urllib.parse.quote(error, safe='')}"
        )
    else:
        callback = (
            f"{redirect}?state={urllib.parse.quote(state, safe='')}"
            f"&code={urllib.parse.quote(code, safe='')}"
        )
    out = os.environ.get("BOB_GCAL_FAKE_BROWSER_OUT")
    try:
        with urllib.request.urlopen(callback, timeout=30) as resp:
            body = resp.read()
            status = getattr(resp, "status", 200)
    except urllib.error.HTTPError as err:
        body = err.read()
        status = err.code
    except Exception as err:
        sys.stderr.write(f"fake browser: {err}\n")
        return 1
    if out:
        path = Path(out)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
        path.with_suffix(path.suffix + ".status").write_text(
            str(status), encoding="utf-8"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
