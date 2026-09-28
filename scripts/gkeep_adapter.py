#!/usr/bin/env python3
"""Pinned Google Keep adapter for `bob gkeep` (adapter protocol v1).

The Rust side writes one compact JSON request to this script's stdin and
closes it. This script writes exactly one JSON response to stdout and logs
to stderr. Every request carries ``"protocol": 1``.

Ops:

- ``ping``: no auth, no network. Reports the interpreter and pinned
  dependency versions.
- ``snapshot``: list Home notes (plus Archive when ``include_archived``),
  authenticated with ``email`` / ``master_token`` / ``device_id``.
- ``archive``: archive notes whose current content still matches the
  ``expect`` object Rust sent back, in one guarded transaction.
- ``exchange``: trade a Google sign-in cookie for a Keep master token.

Run ``python3 -m py_compile`` plus ``--self-test`` via ``just check-adapter``.
"""

# /// script
# requires-python = ">=3.10"
# dependencies = ["gkeepapi==0.17.1", "gpsoauth==2.0.0"]
# [tool.uv]
# exclude-newer = "2026-09-01T00:00:00Z"
# ///

from __future__ import annotations

import contextlib
import datetime
import io
import json
import os
import sys
import tempfile
import threading
import time
import traceback

PROTOCOL_VERSION = 1

STATE_DIR_MODE = 0o700
STATE_FILE_MODE = 0o600

_AUTH_ERROR_NAMES = frozenset(
    {"LoginException", "BrowserLoginRequiredException"}
)
_PROTOCOL_ERROR_NAMES = frozenset(
    {
        "APIException",
        "SyncException",
        "ResyncRequiredException",
        "UpgradeRecommendedException",
        "LabelException",
        "MergeException",
        "InvalidException",
        "ParseException",
        "KeepException",
    }
)
_NETWORK_ERROR_NAMES = frozenset(
    {
        "ConnectionError",
        "ConnectTimeout",
        "ReadTimeout",
        "Timeout",
        "HTTPError",
        "SSLError",
        "ProxyError",
        "TooManyRedirects",
        "RequestException",
    }
)


class AdapterFail(Exception):
    """A controlled adapter failure: print an ``ok:false`` response."""

    def __init__(self, kind: str, message: str) -> None:
        super().__init__(message)
        self.kind = kind
        self.message = message


def fail_response(kind: str, message: str) -> dict:
    """Shape an ``ok:false`` protocol response."""
    return {"ok": False, "error": {"kind": kind, "message": message}}


def scrub(message: str, secrets: list[str]) -> str:
    """Remove secret values from a message; tokens never leak."""
    for secret in secrets:
        if secret:
            message = message.replace(secret, "<redacted>")
    return message


def classify_error(exc: BaseException) -> tuple[str, str]:
    """Map an exception to an ``(error kind, message)`` pair."""
    if isinstance(exc, ImportError):
        return ("dependency", f"missing Python dependency: {exc}")
    name = type(exc).__name__
    module = type(exc).__module__ or ""
    code = getattr(exc, "code", None)
    message = str(exc) or name
    if code in (401, 403) or name in _AUTH_ERROR_NAMES:
        return ("auth", message)
    if code == 429:
        return ("rate_limit", message)
    if module.startswith("requests") or name in _NETWORK_ERROR_NAMES:
        return ("network", message)
    if name in _PROTOCOL_ERROR_NAMES:
        return ("protocol", message)
    return ("internal", f"{name}: {message}")


def _is_list(note: object) -> bool:
    """Whether a note holds a checklist (gkeepapi ``List`` or a stub)."""
    cls = type(note).__name__
    if cls == "List":
        return True
    if cls == "Note":
        return False
    return hasattr(note, "items")


def _live_items(note: object) -> list:
    """Display-order items, excluding deleted or trashed ones."""
    try:
        raw = list(note.items)  # type: ignore[union-attr]
    except (AttributeError, TypeError):
        return []
    return [
        item
        for item in raw
        if not getattr(item, "deleted", False)
        and not getattr(item, "trashed", False)
    ]


def content_of(note: object) -> dict:
    """Build the exact ``KeepContent`` dict for a note.

    There is no normalization here; Rust owns rendering and hashing.
    """
    title = getattr(note, "title", "") or ""
    if _is_list(note):
        items = [
            {
                "text": getattr(item, "text", "") or "",
                "checked": bool(getattr(item, "checked", False)),
                "indented": bool(getattr(item, "indented", False)),
            }
            for item in _live_items(note)
        ]
        return {"title": title, "text": "", "items": items}
    return {"title": title, "text": getattr(note, "text", "") or "", "items": []}


def decide_archive(note: object | None, expect: dict) -> tuple[str, bool]:
    """Decide one note's archive outcome before syncing.

    Returns ``(status, should_archive)`` where ``status`` is ``"proceed"``
    when the note is ready to archive, else a final per-note status.
    """
    if note is None:
        return ("missing", False)
    if getattr(note, "trashed", False) or getattr(note, "deleted", False):
        return ("missing", False)
    if getattr(note, "archived", False):
        return ("already_archived", False)
    if content_of(note) != expect:
        return ("changed", False)
    return ("proceed", True)


def _iso(moment: object) -> str | None:
    """Format a timestamp as ``YYYY-MM-DDTHH:MM:SSZ`` (UTC)."""
    if moment is None or not hasattr(moment, "astimezone"):
        return None
    if getattr(moment, "tzinfo", None) is None:
        moment = moment.replace(tzinfo=datetime.timezone.utc)  # type: ignore[union-attr]
    return moment.astimezone(datetime.timezone.utc).strftime(  # type: ignore[union-attr]
        "%Y-%m-%dT%H:%M:%SZ"
    )


def _attachment_kind(blob: object) -> str:
    cls = type(blob).__name__
    if cls == "NodeImage":
        return "image"
    if cls == "NodeDrawing":
        return "drawing"
    if cls == "NodeAudio":
        return "audio"
    return "other"


def serialize_note(note: object) -> dict:
    """Serialize a gkeepapi note to the ``KeepNote`` protocol shape."""
    stamps = getattr(note, "timestamps", None)
    edited = _iso(getattr(stamps, "edited", None)) or _iso(
        getattr(stamps, "updated", None)
    )
    labels = []
    for label in getattr(getattr(note, "labels", None), "all", lambda: [])():
        name = getattr(label, "name", "")
        if name:
            labels.append(name)
    collaborators = getattr(note, "collaborators", None)
    try:
        shared = len(collaborators) > 0 if collaborators is not None else False
    except TypeError:
        shared = False
    attachments = []
    for blob in getattr(note, "blobs", []) or []:
        inner = getattr(blob, "blob", None)
        text = getattr(inner, "extracted_text", None) or None
        attachments.append(
            {"kind": _attachment_kind(inner), "extracted_text": text}
        )
    return {
        "id": getattr(note, "id", ""),
        "server_id": getattr(note, "server_id", None),
        "kind": "list" if _is_list(note) else "note",
        "content": content_of(note),
        "pinned": bool(getattr(note, "pinned", False)),
        "archived": bool(getattr(note, "archived", False)),
        "shared": shared,
        "labels": labels,
        "attachments": attachments,
        "created": _iso(getattr(stamps, "created", None)),
        "edited": edited,
        "url": getattr(note, "url", None),
    }


def load_state(path: str) -> dict | None:
    """Load the cached Keep state, or ``None`` when absent or corrupt."""
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (FileNotFoundError, NotADirectoryError, OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def save_state(keep: object, path: str) -> None:
    """Write ``keep.dump()`` atomically: temp file, 0600, same-dir rename."""
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, mode=STATE_DIR_MODE, exist_ok=True)
        try:
            os.chmod(parent, STATE_DIR_MODE)
        except OSError:
            pass
    target_dir = parent or "."
    fd, tmp = tempfile.mkstemp(
        dir=target_dir, prefix=".state-", suffix=".tmp"
    )
    try:
        os.fchmod(fd, STATE_FILE_MODE)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(keep.dump(), handle)  # type: ignore[union-attr]
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def _auth_error(exc: BaseException) -> bool:
    name = type(exc).__name__
    code = getattr(exc, "code", None)
    return code in (401, 403) or name in _AUTH_ERROR_NAMES


def connect(auth: dict) -> tuple[object, list[str]]:
    """Authenticate to Keep, retrying once without cached state.

    A corrupt cache, a resync demand, or any non-auth error drops the
    state and retries once with ``state=None``. Auth errors raise at
    once. Returns ``(keep, secrets)``; saves state after connecting.
    """
    import gkeepapi

    email = auth["email"]
    token = auth["master_token"]
    device_id = auth["device_id"]
    state_path = auth["state_path"]
    secrets = [token]

    state = load_state(state_path)
    keep = gkeepapi.Keep()
    try:
        keep.authenticate(
            email, token, state=state, sync=True, device_id=device_id
        )
    except Exception as first:  # noqa: BLE001 - classified below
        if _auth_error(first) or state is None:
            raise
        keep = gkeepapi.Keep()
        keep.authenticate(
            email, token, state=None, sync=True, device_id=device_id
        )
    try:
        save_state(keep, state_path)
    except OSError as exc:
        raise AdapterFail(
            "internal", f"could not write the Keep state cache: {exc}"
        ) from exc
    return keep, secrets


def require_auth(request: dict) -> dict:
    """Pull and validate the auth fields every authed op carries."""
    missing = [
        key
        for key in ("email", "master_token", "device_id", "state_path")
        if not request.get(key)
    ]
    if missing:
        raise AdapterFail(
            "protocol",
            f"request is missing required field(s): {', '.join(missing)}",
        )
    return {
        "email": request["email"],
        "master_token": request["master_token"],
        "device_id": request["device_id"],
        "state_path": request["state_path"],
    }


def _package_version(name: str) -> str:
    """Best-effort version lookup: ``"unknown"`` when it fails."""
    try:
        from importlib import metadata

        return metadata.version(name)
    except Exception:  # noqa: BLE001 - version lookup is best effort
        return "unknown"


def op_ping(request: dict) -> dict:
    """No auth, no network: report interpreter and dependency versions."""
    gkeepapi_version = _package_version("gkeepapi")
    gpsoauth_version = _package_version("gpsoauth")
    return {
        "ok": True,
        "protocol": PROTOCOL_VERSION,
        "python": ".".join(str(part) for part in sys.version_info[:3]),
        "gkeepapi": gkeepapi_version,
        "gpsoauth": gpsoauth_version,
    }


def op_snapshot(request: dict) -> dict:
    """List Home notes, plus Archive when ``include_archived`` is set."""
    auth = require_auth(request)
    include_archived = bool(request.get("include_archived", False))
    try:
        keep, secrets = connect(auth)
        try:
            found = list(
                keep.find(  # type: ignore[union-attr]
                    archived=None if include_archived else False,
                    trashed=False,
                )
            )
            notes = [
                serialize_note(note)
                for note in found
                if not getattr(note, "deleted", False)
                and not getattr(note, "trashed", False)
            ]
            save_state(keep, auth["state_path"])
        except AdapterFail:
            raise
        except Exception as exc:  # noqa: BLE001 - classified below
            raise _report_internal(exc, secrets) from exc
    except AdapterFail:
        raise
    except Exception as exc:  # noqa: BLE001 - classified below
        raise _report_internal(exc, [auth["master_token"]]) from exc
    return {"ok": True, "account": auth["email"], "notes": notes}


def op_archive(request: dict) -> dict:
    """Archive notes whose content still matches, guarded per note."""
    auth = require_auth(request)
    targets = request.get("notes")
    if not isinstance(targets, list):
        raise AdapterFail("protocol", "archive request needs a notes list")
    for target in targets:
        if (
            not isinstance(target, dict)
            or not target.get("id")
            or not isinstance(target.get("expect"), dict)
        ):
            raise AdapterFail(
                "protocol",
                "each archive target needs an id and an expect object",
            )
    try:
        keep, secrets = connect(auth)
        try:
            pending: list[tuple[str, object]] = []
            results: list[dict] = []
            for target in targets:
                note = keep.get(target["id"])  # type: ignore[union-attr]
                status, proceed = decide_archive(note, target["expect"])
                if proceed:
                    note.archived = True  # type: ignore[union-attr]
                    pending.append((target["id"], note))
                else:
                    results.append({"id": target["id"], "status": status})
            keep.sync()  # type: ignore[union-attr]
            keep.sync()  # type: ignore[union-attr]
            for note_id, _note in pending:
                fresh = keep.get(note_id)  # type: ignore[union-attr]
                if fresh is not None and (
                    getattr(fresh, "archived", False) is True
                ):
                    results.append({"id": note_id, "status": "archived"})
                else:
                    results.append(
                        {
                            "id": note_id,
                            "status": "error",
                            "detail": "archive did not persist after sync",
                        }
                    )
            save_state(keep, auth["state_path"])
        except AdapterFail:
            raise
        except Exception as exc:  # noqa: BLE001 - classified below
            raise _report_internal(exc, secrets) from exc
    except AdapterFail:
        raise
    except Exception as exc:  # noqa: BLE001 - classified below
        raise _report_internal(exc, [auth["master_token"]]) from exc
    return {"ok": True, "results": results}


def op_exchange(request: dict) -> dict:
    """Trade a Google sign-in cookie for a Keep master token."""
    for key in ("email", "oauth_token", "device_id"):
        if not request.get(key):
            raise AdapterFail(
                "protocol", f"exchange request needs {key!r}"
            )
    cookie = request["oauth_token"]
    try:
        import gpsoauth

        response = gpsoauth.exchange_token(
            request["email"], cookie, request["device_id"]
        )
    except Exception as exc:  # noqa: BLE001 - classified below
        raise _report_internal(exc, [cookie]) from exc
    if isinstance(response, dict) and response.get("Error"):
        raise AdapterFail(
            "auth", scrub(str(response["Error"]), [cookie])
        )
    token = response.get("Token") if isinstance(response, dict) else None
    if not token:
        raise AdapterFail(
            "internal", "the sign-in exchange returned no master token"
        )
    return {"ok": True, "master_token": token}


OPS = {
    "ping": op_ping,
    "snapshot": op_snapshot,
    "archive": op_archive,
    "exchange": op_exchange,
}


def _parent_alive(pid: int) -> bool:
    """Whether the parent process still exists (for the watchdog)."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return True
    return True


def _maybe_start_parent_watchdog() -> None:
    """Exit when the Rust parent is gone (Ctrl-C orphan fix).

    `spawn_adapter` sets `BOB_GKEEP_PARENT_PID`; a daemon thread checks
    every 0.5s and exits when the parent disappears. Unset or invalid
    values start no watchdog.
    """
    raw = os.environ.get("BOB_GKEEP_PARENT_PID")
    if not raw:
        return
    try:
        pid = int(raw)
    except ValueError:
        return
    if pid <= 0:
        return

    def _watch() -> None:
        while True:
            time.sleep(0.5)
            if not _parent_alive(pid):
                os._exit(1)

    thread = threading.Thread(target=_watch, daemon=True)
    thread.start()


def _report_internal(exc: BaseException, secrets: list[str]) -> AdapterFail:
    """Shape a classified error, printing a traceback for internal errors."""
    kind, message = classify_error(exc)
    if kind == "internal" and sys.exc_info()[0] is not None:
        sys.stderr.write(scrub(traceback.format_exc(), secrets))
        sys.stderr.write("\n")
    return AdapterFail(kind, scrub(message, secrets))


def validate_request(data: object) -> dict:
    """Check the request envelope and return it, or raise ``AdapterFail``."""
    if not isinstance(data, dict):
        raise AdapterFail("protocol", "adapter request must be a JSON object")
    protocol = data.get("protocol")
    if protocol != PROTOCOL_VERSION:
        raise AdapterFail(
            "protocol",
            f"unsupported protocol {protocol!r}: this adapter speaks "
            f"protocol {PROTOCOL_VERSION}",
        )
    op = data.get("op")
    if not isinstance(op, str) or op not in OPS:
        raise AdapterFail(
            "protocol",
            f"unknown op {op!r}: expected one of "
            f"{', '.join(sorted(OPS))}",
        )
    return data


def dispatch(data: dict) -> dict:
    """Run one validated request and return its response object."""
    return OPS[data["op"]](data)


def self_test() -> int:
    """Exercise the offline paths: no network and no Keep account."""
    failures: list[str] = []

    def check(label: str, condition: bool) -> None:
        if not condition:
            failures.append(label)

    class Item:
        def __init__(
            self,
            text: str,
            checked: bool = False,
            indented: bool = False,
            deleted: bool = False,
            trashed: bool = False,
        ) -> None:
            self.text = text
            self.checked = checked
            self.indented = indented
            self.deleted = deleted
            self.trashed = trashed

    class StubNote:
        def __init__(self, title: str, text: str) -> None:
            self.title = title
            self.text = text

    class StubList:
        def __init__(self, title: str, items: list) -> None:
            self.title = title
            self.items = items

    note = StubNote("Call dentist", "They close at 5")
    check(
        "content_of note",
        content_of(note)
        == {"title": "Call dentist", "text": "They close at 5", "items": []},
    )

    items = [
        Item("wood screws"),
        Item("sandpaper", checked=True),
        Item("nested", indented=True),
        Item("gone", deleted=True),
        Item("binned", trashed=True),
    ]
    listing = StubList("Hardware store", items)
    content = content_of(listing)
    check("content_of list title", content["title"] == "Hardware store")
    check("content_of list text is empty", content["text"] == "")
    check(
        "content_of list items",
        content["items"]
        == [
            {"text": "wood screws", "checked": False, "indented": False},
            {"text": "sandpaper", "checked": True, "indented": False},
            {"text": "nested", "checked": False, "indented": True},
        ],
    )

    expect = content_of(listing)
    status, proceed = decide_archive(listing, expect)
    check("decide_archive proceed", (status, proceed) == ("proceed", True))
    check(
        "decide_archive changed",
        decide_archive(listing, {"title": "x", "text": "", "items": []})
        == ("changed", False),
    )
    listing.archived = True  # type: ignore[attr-defined]
    check(
        "decide_archive already_archived",
        decide_archive(listing, expect) == ("already_archived", False),
    )
    check("decide_archive missing", decide_archive(None, expect)[0] == "missing")

    try:
        validate_request({"protocol": 999, "op": "ping"})
        check("protocol-version rejection", False)
    except AdapterFail as exc:
        check("protocol-version rejection", exc.kind == "protocol")

    try:
        validate_request({"protocol": 1, "op": "bogus"})
        check("unknown-op rejection", False)
    except AdapterFail as exc:
        check("unknown-op rejection", exc.kind == "protocol")

    try:
        validate_request({"protocol": 1, "op": "ping"})
    except AdapterFail:
        check("valid request passes", False)

    response = fail_response("auth", "bad token")
    check(
        "error-response shape",
        response
        == {"ok": False, "error": {"kind": "auth", "message": "bad token"}},
    )

    kind, _message = classify_error(ImportError("no module gkeepapi"))
    check("dependency mapping", kind == "dependency")

    class FakeLogin(Exception):
        pass

    FakeLogin.__name__ = "LoginException"
    kind, _message = classify_error(FakeLogin("bad"))
    check("auth mapping", kind == "auth")

    ping = op_ping({"protocol": 1, "op": "ping"})
    check(
        "ping shape",
        ping.get("ok") is True
        and ping.get("protocol") == PROTOCOL_VERSION
        and all(key in ping for key in ("python", "gkeepapi", "gpsoauth")),
    )

    check(
        "token scrubbing",
        scrub("token aas_et/secret here", ["aas_et/secret"])
        == "token <redacted> here",
    )

    class UnknownBlob:
        pass

    class NoneBlob:
        blob = None

    check("unknown attachment kind is other", _attachment_kind(object()) == "other")
    check(
        "none attachment kind is other",
        _attachment_kind(type("B", (), {})()) == "other",
    )
    check("unknown blob instance is other", _attachment_kind(UnknownBlob()) == "other")

    try:
        validate_request({"protocol": 1, "op": ["ping"]})
        check("non-string op rejection", False)
    except AdapterFail as exc:
        check("non-string op rejection", exc.kind == "protocol")

    check(
        "missing package version is unknown",
        _package_version("definitely-not-a-real-package-xyz") == "unknown",
    )

    secret = "aas_et/self-test-secret"
    try:
        raise ValueError(f"boom {secret}")
    except ValueError as exc:
        buf = io.StringIO()
        with contextlib.redirect_stderr(buf):
            internal = _report_internal(exc, [secret])
        captured = buf.getvalue()
        check(
            "internal error response shape",
            isinstance(internal, AdapterFail)
            and internal.kind == "internal"
            and "<redacted>" in internal.message
            and secret not in internal.message,
        )
        check(
            "internal traceback is scrubbed",
            "<redacted>" in captured and secret not in captured,
        )
        check(
            "internal stderr is not empty",
            "ValueError" in captured or "Traceback" in captured,
        )

    check("_parent_alive self", _parent_alive(os.getpid()) is True)
    import subprocess

    with subprocess.Popen(["true"]) as child:
        dead_pid = child.pid
    # Reaped after context exit.
    check("_parent_alive dead", _parent_alive(dead_pid) is False)

    if failures:
        for failure in failures:
            print(f"self-test failure: {failure}", file=sys.stderr)
        return 1
    print("ok")
    return 0


def main(argv: list[str]) -> int:
    """Read one request from stdin, write one response to stdout."""
    if "--self-test" in argv[1:]:
        return self_test()
    _maybe_start_parent_watchdog()
    try:
        raw = sys.stdin.read()
    except Exception as exc:  # noqa: BLE001 - stdin is best effort
        print(json.dumps(fail_response("internal", f"could not read stdin: {exc}")))
        return 0
    try:
        data = json.loads(raw)
    except ValueError as exc:
        print(
            json.dumps(
                fail_response("protocol", f"adapter request was not valid JSON: {exc}")
            )
        )
        return 0
    try:
        print(json.dumps(dispatch(validate_request(data))))
        return 0
    except AdapterFail as fail:
        print(json.dumps(fail_response(fail.kind, fail.message)))
        return 0
    except Exception as exc:  # noqa: BLE001 - unexpected: traceback + ok:false
        secrets: list[str] = []
        if isinstance(data, dict):
            for key in ("master_token", "oauth_token"):
                value = data.get(key)
                if isinstance(value, str) and value:
                    secrets.append(value)
        kind, message = classify_error(exc)
        if kind == "internal":
            sys.stderr.write(scrub(traceback.format_exc(), secrets))
            sys.stderr.write("\n")
        print(json.dumps(fail_response(kind, scrub(message, secrets))))
        return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
