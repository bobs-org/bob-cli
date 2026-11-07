#!/usr/bin/env python3
"""Fake gog for bob gmail tests. Records argv; never talks to Google."""
from __future__ import annotations

import json
import base64
import hashlib
import os
import sys
import time
from pathlib import Path
from urllib.parse import parse_qs, quote, urlparse

CLIENT = "bob-gmail"
READONLY_SCOPE = "https://www.googleapis.com/auth/gmail.readonly"
COMPOSE_SCOPE = "https://www.googleapis.com/auth/gmail.compose"
EMAIL = "you@example.com"

FLAGS_WITH_VALUE = {
    "--color",
    "--client",
    "--account",
    "--services",
    "--from",
    "--to",
    "--max",
    "--page",
    "--fields",
    "--format",
    "--params",
    "--scope",
    "--auth-url",
    "--step",
    "--redirect-uri",
    "--timeout",
    "--listen-addr",
    "--redirect-host",
    "--gmail-scope",
    "--extra-scopes",
    "--enable-commands",
    "--enable-commands-exact",
    "--disable-commands",
    "--out",
    "--name",
    "--to",
    "--cc",
    "--bcc",
    "--subject",
    "--body",
    "--body-file",
    "--body-html",
    "--body-html-file",
    "--reply-to-message-id",
    "--thread-id",
    "--reply-to",
    "--signature-from",
    "--signature-file",
    "--attach",
}

args = sys.argv[1:]
log_path = os.environ.get("BOB_GMAIL_FAKE_LOG")
state_path = os.environ.get("BOB_GMAIL_FAKE_STATE")
scenario = os.environ.get("BOB_GMAIL_FAKE_SCENARIO", "ok")
fake_home = Path(os.environ.get("BOB_GMAIL_FAKE_HOME", "/tmp"))
stdin_log = os.environ.get("BOB_GMAIL_FAKE_STDIN_LOG")

if log_path:
    path = Path(log_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(args) + "\n")


def positional(argv: list[str]) -> list[str]:
    out: list[str] = []
    i = 0
    while i < len(argv):
        item = argv[i]
        if item == "-":
            out.append(item)
            i += 1
            continue
        if item in FLAGS_WITH_VALUE:
            i += 2
            continue
        if item.startswith("-"):
            i += 1
            continue
        out.append(item)
        i += 1
    return out


def flag_value(name: str) -> str | None:
    for index, item in enumerate(args):
        if item == name and index + 1 < len(args):
            return args[index + 1]
        if item.startswith(name + "="):
            return item[len(name) + 1 :]
    return None


def flag_values(name: str) -> list[str]:
    values: list[str] = []
    index = 0
    while index < len(args):
        item = args[index]
        if item == name and index + 1 < len(args):
            values.append(args[index + 1])
            index += 2
            continue
        if item.startswith(name + "="):
            values.append(item[len(name) + 1 :])
        index += 1
    return values


def load_state() -> dict:
    if state_path and Path(state_path).exists():
        try:
            return json.loads(Path(state_path).read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return {}
    return {}


def save_state(state: dict) -> None:
    if not state_path:
        return
    path = Path(state_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(state), encoding="utf-8")


def connected_account(email: str = EMAIL, scopes: list[str] | None = None) -> dict:
    return {
        "email": email,
        "client": CLIENT,
        "services": ["gmail"],
        "scopes": scopes if scopes is not None else [READONLY_SCOPE],
    }


def scenario_defaults(name: str) -> dict:
    home = fake_home
    try:
        home.mkdir(parents=True, exist_ok=True)
    except Exception:
        pass
    bob_path = str(home / "bob-gmail.json")
    defaults: dict = {
        "version": "0.44.0",
        "clients": [
            {"client": CLIENT, "path": bob_path, "default": True}
        ],
        "accounts": [connected_account()],
    }
    if name == "too_old":
        defaults["version"] = "0.43.0"
    elif name == "v_prefixed_version":
        # Real gog reports its version token with a leading `v`.
        defaults["version"] = "v0.44.0"
    elif name == "no_client":
        defaults["clients"] = []
        defaults["accounts"] = []
    elif name == "not_logged_in":
        defaults["accounts"] = []
    elif name == "wrong_account":
        defaults["accounts"] = [connected_account("other@example.com")]
    elif name == "compose_grant":
        defaults["accounts"] = [
            connected_account(EMAIL, [READONLY_SCOPE, COMPOSE_SCOPE])
        ]
    elif name == "compose_grant_gog_0471":
        # gog 0.47.1 marks given HTML with its direction and writes named
        # recipients in Go's mailbox form.
        defaults["version"] = "0.47.1"
        defaults["accounts"] = [
            connected_account(EMAIL, [READONLY_SCOPE, COMPOSE_SCOPE])
        ]
        defaults["direction_html"] = True
        defaults["go_mailbox_format"] = True
    elif name == "compose_withheld":
        # The user leaves Google's compose checkbox unticked at consent.
        defaults["withhold_compose"] = True
    elif name == "no_scope":
        defaults["accounts"] = [
            {
                "email": EMAIL,
                "client": CLIENT,
                "services": [],
                "scopes": [
                    "https://www.googleapis.com/auth/calendar.readonly"
                ],
            }
        ]
    elif name == "invalid_grant":
        defaults["fail"] = {
            "search": {
                "stderr": "invalid_grant: Token has been expired or revoked",
                "code": 1,
            }
        }
    elif name == "api_disabled":
        defaults["fail"] = {
            "search": {
                "stderr": (
                    "accessNotConfigured: Gmail API has not been used in "
                    "project 123456789012"
                ),
                "code": 1,
            }
        }
    elif name == "exchange_invalid_grant":
        defaults["accounts"] = []
        defaults["fail"] = {
            "auth_finish": {
                "stderr": "invalid_grant: Token has been expired or revoked",
                "code": 1,
            }
        }
    elif name == "two_bob_accounts":
        defaults["accounts"] = [
            connected_account(),
            connected_account("work@example.com"),
        ]
    elif name == "auth_expiry":
        defaults["fail"] = {
            operation: {
                "stderr": "invalid_grant: PRIVATE_SENTINEL expired token",
                "code": 1,
            }
            for operation in (
                "search",
                "thread_get",
                "message_get",
                "labels_list",
                "api_call",
            )
        }
    elif name == "not_found":
        defaults["fail"] = {
            operation: {
                "stderr": "notFound: PRIVATE_SENTINEL Gmail item missing",
                "code": 1,
            }
            for operation in ("search", "thread_get", "message_get", "api_call")
        }
    elif name == "poison_error":
        defaults["fail"] = {
            "thread_get": {
                "stderr": "unclassified failure PRIVATE_SENTINEL body text",
                "code": 1,
            },
            "labels_list": {
                "stderr": "unclassified failure PRIVATE_SENTINEL label text",
                "code": 1,
            },
        }
    return defaults


def dump(payload: object) -> None:
    sys.stdout.write(json.dumps(payload) + "\n")


def fail(message: str, code: int = 1) -> None:
    sys.stderr.write(message + "\n")
    raise SystemExit(code)


def valid_client_payload(payload: bytes) -> bool:
    try:
        value = json.loads(payload.decode("utf-8"))
    except Exception:
        return False
    for key in ("installed", "web"):
        section = value.get(key)
        if isinstance(section, dict):
            client_id = section.get("client_id")
            secret = section.get("client_secret")
            return bool(
                isinstance(client_id, str)
                and client_id.strip()
                and isinstance(secret, str)
                and secret.strip()
            )
    return False


state = load_state()
defaults = scenario_defaults(scenario)
for key, value in defaults.items():
    state.setdefault(key, value)

pos = positional(args)
client_flag = flag_value("--client")


def op_name() -> str:
    if pos[:1] == ["version"]:
        return "version"
    if pos[:3] == ["auth", "credentials", "list"]:
        return "credentials_list"
    if pos[:3] == ["auth", "credentials", "set"]:
        return "credentials_set"
    if pos[:2] == ["auth", "list"]:
        return "accounts"
    if pos[:2] == ["auth", "add"]:
        return "auth_add"
    if pos[:2] == ["auth", "remove"]:
        return "auth_remove"
    if pos[:2] == ["gmail", "search"]:
        return "search"
    if pos[:2] == ["gmail", "drafts"]:
        if len(pos) > 2 and pos[2] in {"create", "update", "get"}:
            return "draft_" + pos[2]
        if len(pos) > 2 and pos[2] == "send":
            return "draft_send"
        return "forbidden_send"
    if pos[:3] == ["gmail", "thread", "get"]:
        return "thread_get"
    if pos[:3] == ["gmail", "threads", "get"]:
        return "thread_get"
    if pos[:2] == ["gmail", "get"]:
        return "message_get"
    if pos[:2] == ["gmail", "url"]:
        return "url"
    if pos[:2] == ["api", "call"]:
        return "api_call"
    if pos[:2] == ["gmail", "attachment"]:
        return "attachment"
    if pos[:3] == ["gmail", "labels", "list"]:
        return "labels_list"
    if pos[:3] == ["gmail", "messages", "search"]:
        return "search"
    if pos[:1] == ["gmail"]:
        if len(pos) > 1 and pos[1] in {"send", "reply"}:
            return "forbidden_send"
        return "gmail_other"
    return "unknown"


op = op_name()

fail_spec = (state.get("fail") or {}).get(op)
if fail_spec:
    fail(fail_spec.get("stderr") or "gog failed", int(fail_spec.get("code") or 1))

if op == "version":
    dump(
        {
            "version": state.get("version", "0.44.0"),
            "commit": "test",
            "date": "2026-11-05",
        }
    )
    raise SystemExit(0)

if op == "credentials_list":
    dump({"clients": state.get("clients") or []})
    raise SystemExit(0)

if op == "credentials_set":
    path = pos[3] if len(pos) > 3 else ""
    if path == "-":
        payload = sys.stdin.buffer.read()
        if stdin_log:
            log = Path(stdin_log)
            log.parent.mkdir(parents=True, exist_ok=True)
            with log.open("ab") as handle:
                handle.write(payload + b"\n")
        if not valid_client_payload(payload):
            fail(
                "invalid credentials.json "
                "(expected installed/web client_id and client_secret)",
                1,
            )
        stored_path = str(fake_home / "bob-gmail.json")
        try:
            Path(stored_path).write_bytes(payload)
        except Exception:
            pass
        path = stored_path
    else:
        try:
            payload = Path(path).read_bytes()
        except Exception:
            fail(
                "invalid credentials.json "
                "(expected installed/web client_id and client_secret)",
                1,
            )
            raise SystemExit(1)
        if not valid_client_payload(payload):
            fail(
                "invalid credentials.json "
                "(expected installed/web client_id and client_secret)",
                1,
            )
    clients = list(state.get("clients") or [])
    found = False
    for entry in clients:
        if entry.get("client") == CLIENT:
            entry["path"] = path
            found = True
            break
    if not found:
        clients.append({"client": CLIENT, "path": path, "default": False})
    state["clients"] = clients
    save_state(state)
    dump({"ok": True})
    raise SystemExit(0)

if op == "accounts":
    accounts = list(state.get("accounts") or [])
    if client_flag:
        accounts = [
            entry for entry in accounts if entry.get("client") == client_flag
        ]
    dump({"accounts": accounts})
    raise SystemExit(0)

def draft_header(message: dict, name: str) -> str | None:
    payload = message.get("payload") or {}
    for header in payload.get("headers") or []:
        if header.get("name", "").lower() == name.lower():
            return header.get("value")
    return None


def go_mailbox(value: str) -> str:
    """Render one recipient the way Go's mail.Address.String() does."""
    value = value.strip()
    if "<" not in value or not value.endswith(">"):
        return value
    name, address = value.rsplit("<", 1)
    name = name.strip().strip('"')
    address = address[:-1].strip()
    return f'"{name}" <{address}>' if name else address


def split_recipients(value: str | None) -> list[str]:
    if not value:
        return []
    return [piece.strip() for piece in value.split(",") if piece.strip()]


def private_file(flag: str) -> tuple[Path, bytes] | None:
    raw = flag_value(flag)
    if raw is None:
        return None
    path = Path(raw)
    try:
        mode = path.stat().st_mode & 0o777
        parent_mode = path.parent.stat().st_mode & 0o777
        if mode != 0o600 or parent_mode & 0o077:
            fail("fake gog: draft body or attachment staging is not private", 2)
        return path, path.read_bytes()
    except OSError:
        fail("fake gog: draft body or attachment file is unreadable", 2)
    return None


def b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")


def mime_text(mime: str, raw: bytes) -> dict:
    return {
        "mimeType": mime,
        "body": {"size": len(raw), "data": b64url(raw)},
    }


def build_draft_message(
    draft: dict,
    create: bool,
    compose: bool,
) -> dict:
    previous = draft.get("message") or {}
    plain_file = private_file("--body-file")
    html_file = private_file("--body-html-file")
    if plain_file is None or html_file is None:
        fail("fake gog: draft create/update requires private plain and HTML body files", 2)
    plain = plain_file[1]
    html = html_file[1]
    if state.get("direction_html") and html.strip():
        # gog 0.47.1+ puts given HTML, trimmed, inside one <div dir="auto">.
        html = b'<div dir="auto">' + html.strip() + b"</div>"
    if "--signature" in args:
        signature = (state.get("signature") or "-- \nTest signature").encode()
        plain = plain.rstrip(b"\r\n") + b"\n\n" + signature
        html += b'<div class="gmail_signature">Test signature</div>'

    to_opt = flag_value("--to")
    cc_opt = flag_value("--cc")
    bcc_opt = flag_value("--bcc")
    if create:
        to = split_recipients(to_opt)
        cc = split_recipients(cc_opt)
        bcc = split_recipients(bcc_opt)
    else:
        to = split_recipients(to_opt) if to_opt is not None else list(draft.get("to") or [])
        cc = split_recipients(cc_opt) if cc_opt is not None else list(draft.get("cc") or [])
        bcc = split_recipients(bcc_opt) if bcc_opt is not None else list(draft.get("bcc") or [])

    reply_id = flag_value("--reply-to-message-id") if create else None
    thread_id = previous.get("threadId") or "0123456789abcdef"
    in_reply_to = draft.get("in_reply_to")
    references = draft.get("references")
    if create and reply_id:
        thread_envelope = state.get("thread") or {}
        thread = thread_envelope.get("thread") or thread_envelope
        target = next(
            (item for item in thread.get("messages", []) if item.get("id") == reply_id),
            None,
        )
        if target is None:
            fail("notFound: synthetic reply target missing", 1)
        thread_id = thread.get("id") or target.get("threadId") or "thread-reply-1"
        subject = draft_header(target, "Subject") or ""
        target_from = draft_header(target, "From") or ""
        target_to = split_recipients(draft_header(target, "To"))
        target_cc = split_recipients(draft_header(target, "Cc"))
        if "--reply-all" in args:
            to = [target_from] if target_from else []
            cc = [value for value in target_to + target_cc if EMAIL not in value]
        elif not to:
            to = [target_from] if target_from else []
        in_reply_to = draft_header(target, "Message-ID")
        references = " ".join(
            value
            for value in [draft_header(target, "References"), in_reply_to]
            if value
        )
    if not create and "--clear-reply-context" in args:
        in_reply_to = None
        references = None
        thread_id = "0123456789abcdef"

    attachment_options = flag_values("--attach")
    if create:
        attachment_paths = attachment_options
    elif "--clear-attachments" in args:
        attachment_paths = []
    elif attachment_options:
        attachment_paths = attachment_options
    else:
        attachment_paths = [item["path"] for item in (draft.get("attachments") or [])]

    attachment_parts = []
    attachments: list[dict] = []
    for raw_path in attachment_paths:
        staged = private_file_for_path(raw_path)
        if staged is None:
            fail("fake gog: private attachment snapshot is unreadable", 2)
        path, content = staged
        name = path.name.split("_", 1)[-1]
        attachment_parts.append(
            {
                "mimeType": "application/octet-stream",
                "filename": name,
                "headers": [
                    {"name": "Content-Disposition", "value": f'attachment; filename="{name}"'}
                ],
                "body": {"size": len(content), "data": b64url(content)},
            }
        )
        attachments.append({"path": raw_path, "name": name, "content": b64url(content)})

    alternative = {
        "mimeType": "multipart/alternative",
        "parts": [mime_text("text/plain", plain), mime_text("text/html", html)],
    }
    root = {
        "mimeType": "multipart/mixed" if attachment_parts else "multipart/alternative",
        "parts": [alternative, *attachment_parts] if attachment_parts else alternative["parts"],
        "headers": [],
    }
    subject_value = flag_value("--subject") or ""
    if not create and flag_value("--subject") is None:
        subject_value = draft.get("subject", "")
    headers = [
        {"name": "From", "value": EMAIL},
        {"name": "Subject", "value": subject_value},
    ]
    if state.get("go_mailbox_format"):
        to, cc, bcc = (
            [go_mailbox(value) for value in values] for values in (to, cc, bcc)
        )
    for name, values in (("To", to), ("Cc", cc), ("Bcc", bcc)):
        if values:
            headers.append({"name": name, "value": ", ".join(values)})
    if in_reply_to:
        headers.append({"name": "In-Reply-To", "value": in_reply_to})
    if references:
        headers.append({"name": "References", "value": references})
    root["headers"] = headers
    sequence = int(state.get("draft_message_seq") or 0) + 1
    state["draft_message_seq"] = sequence
    message = {
        "id": f"draft-message-{sequence}",
        "threadId": thread_id,
        "labelIds": ["DRAFT"],
        "payload": root,
    }
    draft.update(
        {
            "message": message,
            "to": to,
            "cc": cc,
            "bcc": bcc,
            "subject": subject_value,
            "attachments": attachments,
            "in_reply_to": in_reply_to,
            "references": references,
        }
    )
    return message


def private_file_for_path(raw_path: str) -> tuple[Path, bytes] | None:
    path = Path(raw_path)
    try:
        if path.stat().st_mode & 0o777 != 0o600 or path.parent.stat().st_mode & 0o077:
            fail("fake gog: attachment snapshot is not private", 2)
        return path, path.read_bytes()
    except OSError:
        return None


if op in {"draft_create", "draft_update", "draft_get"}:
    if "--gmail-no-send" not in args:
        fail("fake gog: every Gmail draft call must include --gmail-no-send", 2)
    if "--thread-id" in args and "--reply-to-message-id" in args:
        fail("fake gog: --thread-id and --reply-to-message-id cannot be combined", 2)
    if op == "draft_get":
        if "--readonly" not in args:
            fail("fake gog: draft get must be read-only", 2)
    elif "--readonly" in args:
        fail("fake gog: draft create/update must not be read-only", 2)
    expected = {
        "draft_create": "gmail.drafts.create",
        "draft_update": "gmail.drafts.update",
        "draft_get": "gmail.drafts.get",
    }[op]
    if flag_value("--enable-commands-exact") != expected:
        fail("fake gog: draft call has an unexpected exact-command allowlist", 2)
    drafts = state.setdefault("drafts", {})
    if op == "draft_create":
        draft_id = f"draft-{int(state.get('draft_seq') or 0) + 1}"
        state["draft_seq"] = int(state.get("draft_seq") or 0) + 1
        draft: dict = {"id": draft_id}
        message = build_draft_message(draft, True, True)
        drafts[draft_id] = draft
        save_state(state)
        dump({"draftId": draft_id, "message": message})
        raise SystemExit(0)
    draft_id = pos[3] if len(pos) > 3 else ""
    draft = drafts.get(draft_id)
    if draft is None:
        fail("notFound: synthetic draft missing", 1)
    if op == "draft_update":
        message = build_draft_message(draft, False, True)
        save_state(state)
        dump({"draftId": draft_id, "message": message})
        raise SystemExit(0)
    dump({"draft": {"id": draft_id, "message": draft.get("message")}})
    raise SystemExit(0)

if op == "draft_send":
    if "--gmail-no-send" in args:
        fail("fake gog: drafts send must omit --gmail-no-send", 2)
    if "--readonly" in args:
        fail("fake gog: drafts send must omit --readonly", 2)
    if flag_value("--enable-commands-exact") != "gmail.drafts.send":
        fail("fake gog: drafts send has an unexpected exact-command allowlist", 2)
    draft_id = pos[3] if len(pos) > 3 else ""
    drafts = state.setdefault("drafts", {})
    draft = drafts.get(draft_id)
    if draft is None:
        fail("notFound: synthetic draft missing", 1)
    previous = draft.get("message") or {}
    thread_id = previous.get("threadId") or "0123456789abcdef"
    to = list(draft.get("to") or [])
    cc = list(draft.get("cc") or [])
    bcc = list(draft.get("bcc") or [])
    subject = draft.get("subject") or ""
    in_reply_to = draft.get("in_reply_to")
    references = draft.get("references")
    seq = int(state.get("sent_seq") or 0) + 1
    state["sent_seq"] = seq
    sent_id = f"{seq:016x}"
    rfc = f"<sent-{seq}@example.test>"
    headers = [
        {"name": "From", "value": EMAIL},
        {"name": "Subject", "value": subject},
    ]
    if to:
        headers.append({"name": "To", "value": ", ".join(to)})
    if cc:
        headers.append({"name": "Cc", "value": ", ".join(cc)})
    if bcc:
        headers.append({"name": "Bcc", "value": ", ".join(bcc)})
    headers.append({"name": "Message-ID", "value": rfc})
    if in_reply_to:
        headers.append({"name": "In-Reply-To", "value": in_reply_to})
    if references:
        headers.append({"name": "References", "value": references})
    message = {
        "id": sent_id,
        "threadId": thread_id,
        "labelIds": ["SENT"],
        "internalDate": str(int(time.time() * 1000)),
        "payload": {
            "mimeType": "text/plain",
            "headers": headers,
            "body": {"size": 5, "data": b64url(b"hello")},
        },
    }
    sent_threads = state.setdefault("sent_threads", {})
    entry = sent_threads.get(thread_id) or {"id": thread_id, "messages": []}
    entry["messages"].append(message)
    sent_threads[thread_id] = entry
    if state.get("thread"):
        thread_envelope = state["thread"]
        thread = thread_envelope.get("thread") or thread_envelope
        if thread.get("id") == thread_id:
            thread.setdefault("messages", []).append(message)
    del drafts[draft_id]
    save_state(state)
    dump({"message": {"id": sent_id, "threadId": thread_id}})
    raise SystemExit(0)

if op == "forbidden_send":
    fail("fake gog: direct send commands are forbidden", 2)

if op == "auth_add":
    if "--remote" not in args:
        fail("fake gog: interactive auth add is not supported", 2)
    step = flag_value("--step") or ""
    email = pos[2] if len(pos) > 2 else EMAIL
    force_consent = "--force-consent" in args
    if step == "1":
        redirect = flag_value("--redirect-uri")
        if not redirect:
            fail("fake gog: remote step 1 requires --redirect-uri")
        cached = state.get("manual_auth") or {}
        reused = bool(
            cached
            and cached.get("client") == CLIENT
            and cached.get("force_consent") is force_consent
            and cached.get("redirect_uri") == redirect
            and cached.get("state")
        )
        if reused:
            token = cached["state"]
        else:
            seq = int(state.get("auth_seq") or 0) + 1
            state["auth_seq"] = seq
            token = f"st/ate+{seq}"
            state["manual_auth"] = {
                "client": CLIENT,
                "force_consent": force_consent,
                "state": token,
                "redirect_uri": redirect,
            }
        auth_url = (
            "https://accounts.google.com/o/oauth2/auth"
            "?access_type=offline"
            "&client_id=123456789012-abc.apps.googleusercontent.com"
            f"&redirect_uri={quote(redirect, safe='')}"
            "&response_type=code"
            f"&state={quote(token, safe='')}"
        )
        save_state(state)
        dump({"auth_url": auth_url, "state_reused": reused})
        raise SystemExit(0)
    if step == "2":
        finish_fail = (state.get("fail") or {}).get("auth_finish")
        if finish_fail:
            fail(
                finish_fail.get("stderr") or "gog failed",
                int(finish_fail.get("code") or 1),
            )
        auth_url = flag_value("--auth-url") or ""
        parsed = urlparse(auth_url)
        query = parse_qs(parsed.query)
        got_state = (query.get("state") or [None])[0]
        got_code = (query.get("code") or [None])[0]
        got_redirect = f"{parsed.scheme}://{parsed.netloc}{parsed.path}"
        flag_redirect = flag_value("--redirect-uri")
        cached = state.get("manual_auth") or {}
        if not cached.get("state"):
            fail(
                "manual auth state missing; start a new manual flow or "
                "run remote step 1 again"
            )
        if cached.get("force_consent") is not force_consent:
            fail("manual auth state mismatch")
        if cached.get("redirect_uri") != got_redirect:
            fail("manual auth state mismatch")
        if flag_redirect and flag_redirect != cached.get("redirect_uri"):
            fail("manual auth state mismatch")
        if not got_state:
            fail("missing state in redirect URL")
        if got_state != cached.get("state"):
            fail(
                "manual auth state missing; start a new manual flow or "
                "run remote step 1 again"
            )
        if not got_code:
            fail("no code found in URL")
        # Grant readonly; add compose when the original step-1 requested it.
        extra = ""
        for i, item in enumerate(args):
            if item == "--extra-scopes" and i + 1 < len(args):
                extra = args[i + 1]
        scopes = [READONLY_SCOPE]
        if COMPOSE_SCOPE in extra and not state.get("withhold_compose"):
            scopes.append(COMPOSE_SCOPE)
        accounts = [
            entry
            for entry in (state.get("accounts") or [])
            if not (
                entry.get("email") == email and entry.get("client") == CLIENT
            )
        ]
        accounts.append(connected_account(email, scopes))
        state["accounts"] = accounts
        state["manual_auth"] = None
        save_state(state)
        dump({"ok": True})
        raise SystemExit(0)
    fail("fake gog: remote auth add requires --step 1 or 2", 2)

if op == "auth_remove":
    email = pos[2] if len(pos) > 2 else ""
    accounts = [
        entry
        for entry in (state.get("accounts") or [])
        if not (entry.get("email") == email and entry.get("client") == CLIENT)
    ]
    state["accounts"] = accounts
    save_state(state)
    dump({"ok": True})
    raise SystemExit(0)

if op == "search":
    query = pos[2] if len(pos) > 2 else ""
    page = flag_value("--page")
    if "in:sent" in query:
        sent_threads = state.get("sent_threads") or {}
        ids = []
        if "rfc822msgid:" in query:
            rfc = query.split("rfc822msgid:", 1)[1].split()[0]
            for thread_id, entry in sent_threads.items():
                for item in entry.get("messages", []):
                    for header in (item.get("payload") or {}).get("headers", []):
                        if header.get("name", "").lower() == "message-id" and header.get("value", "").strip() == rfc.strip():
                            ids.append(thread_id)
                            break
        else:
            ids = list(sent_threads.keys())
        dump(
            {
                "threads": [{"id": thread_id} for thread_id in ids],
                "nextPageToken": None,
                "resultSizeEstimate": len(ids),
            }
        )
    elif scenario in {"gmail_pull_one", "gmail_pull_many"} and query.startswith("label:"):
        ids = ["0123456789abcdef"]
        if scenario == "gmail_pull_many":
            ids.append("aaaaaaaaaaaaaaaa")
        dump(
            {
                "threads": [{"id": thread_id} for thread_id in ids],
                "nextPageToken": None,
                "resultSizeEstimate": len(ids),
            }
        )
    elif scenario == "search_zero" or "zero@example.test" in query:
        dump({"threads": [], "nextPageToken": None, "resultSizeEstimate": 0})
    elif scenario == "search_many" or "many@example.test" in query:
        dump(
            {
                "threads": [
                    {"id": "0123456789abcdef"},
                    {"id": "aaaaaaaaaaaaaaaa"},
                ],
                "nextPageToken": None,
                "resultSizeEstimate": 2,
            }
        )
    elif scenario == "search_one" or "one@example.test" in query:
        dump(
            {
                "threads": [{"id": "0123456789abcdef"}],
                "nextPageToken": None,
                "resultSizeEstimate": 1,
            }
        )
    elif scenario == "search_paginated" or "paged@example.test" in query:
        if page:
            dump(
                {
                    "threads": [],
                    "nextPageToken": None,
                    "resultSizeEstimate": 1,
                }
            )
        else:
            dump(
                {
                    "threads": [{"id": "0123456789abcdef"}],
                    "nextPageToken": "synthetic-page-2",
                    "resultSizeEstimate": 1,
                }
            )
    elif scenario in {"search_malformed", "malformed_json"}:
        sys.stdout.write("{malformed JSON\n")
    else:
        dump({"threads": [], "nextPageToken": None, "resultSizeEstimate": 0})
    raise SystemExit(0)

if op == "api_call":
    import base64

    # The save path uses exactly one Discovery call shape for attachment
    # bytes. Anything else is a regression in the narrow read contract.
    if pos[2:5] != ["gmail", "v1", "gmail.users.messages.attachments.get"]:
        fail(f"fake gog: unexpected api call {pos[2:5]}", 2)
    if "--no-cache" not in args:
        fail("fake gog: api call requires --no-cache", 2)
    scope = flag_value("--scope")
    if scope != READONLY_SCOPE:
        fail(f"fake gog: api call requires readonly scope, got {scope}", 2)
    allowlist = flag_value("--enable-commands") or ""
    allowed = set(allowlist.split(","))
    if {"api.call", "api.gmail.users.messages.attachments.get"} - allowed:
        fail(
            "fake gog: api call requires api.call and "
            "api.gmail.users.messages.attachments.get in --enable-commands",
            2,
        )
    try:
        params = json.loads(flag_value("--params") or "{}")
    except json.JSONDecodeError:
        fail("fake gog: --params is not JSON", 2)
    if params.get("userId") != "me":
        fail("fake gog: --params userId must be me", 2)
    message_id = params.get("messageId") or ""
    attachment_id = params.get("id") or ""
    if set(params) != {"userId", "messageId", "id"}:
        fail("fake gog: --params must hold only userId/messageId/id", 2)
    if scenario == "api_malformed":
        sys.stdout.write("{malformed JSON\n")
        raise SystemExit(0)
    attachments = state.get("attachments") or {}
    entry = attachments.get(f"{message_id}/{attachment_id}")
    if entry is not None:
        dump({"data": entry["data"], "size": entry["size"]})
    else:
        default = base64.urlsafe_b64encode(b"hello").decode("ascii")
        dump({"data": default, "size": 5})
    raise SystemExit(0)

if op == "thread_get":
    state = load_state()
    if "thread" in state:
        dump(state["thread"])
        raise SystemExit(0)
    hex_id = pos[3] if len(pos) > 3 else "1a0b9eb2cc26ab96"
    sent_threads = state.get("sent_threads") or {}
    if hex_id in sent_threads:
        entry = sent_threads[hex_id]
        dump({"thread": {"id": hex_id, "historyId": "1", "messages": entry.get("messages", [])}})
        raise SystemExit(0)
    if scenario == "thread_mismatch":
        hex_id = "aaaaaaaaaaaaaaaa"
    if flag_value("--format") == "metadata":
        if scenario in {"metadata_malformed", "malformed_json"}:
            sys.stdout.write("{malformed JSON\n")
            raise SystemExit(0)
        subject = "Re: Lease renewal #task"
        snippet = "Hello [[world]]; the sender's snippet is inert."
        sender = "Dana Whitfield <dana@example.test>"
        message_count = 2 if hex_id != "aaaaaaaaaaaaaaaa" else 1
        messages = []
        for index in range(message_count):
            messages.append(
                {
                    "id": f"{index + 1:016x}",
                    "threadId": hex_id,
                    "labelIds": ["INBOX"],
                    "internalDate": str(1793908800000 + index * 60000),
                    "snippet": snippet if index == message_count - 1 else "Earlier message",
                    "payload": {
                        "mimeType": "text/plain",
                        "headers": [
                            {"name": "From", "value": sender},
                            {"name": "To", "value": "you@example.com"},
                            {"name": "Subject", "value": subject},
                        ],
                    },
                }
            )
        dump({"thread": {"id": hex_id, "historyId": "2", "messages": messages}})
        raise SystemExit(0)
    if scenario in {
        "thread_nonempty",
        "poison",
        "poison_error",
        "gmail_pull_one",
        "gmail_pull_many",
        "gmail_attachment",
        "gmail_attachment_large",
    }:
        body = "U2Vuc2l0aXZlIHBvaXNvbiBib2R5" if scenario == "poison" else "SGVsbG8="
        from_value = "PRIVATE_SENTINEL <sender@example.com>" if scenario == "poison" else "Ada <ada@example.com>"
        if scenario in {"gmail_attachment", "gmail_attachment_large"}:
            attachment_size = 26 * 1024 * 1024 if scenario == "gmail_attachment_large" else 5
            payload = {
                "mimeType": "multipart/mixed",
                "headers": [
                    {"name": "From", "value": from_value},
                    {"name": "To", "value": EMAIL},
                    {"name": "Subject", "value": "Attachment sample"},
                    {"name": "Message-ID", "value": "<message@example.test>"},
                ],
                "parts": [
                    {
                        "partId": "body-1",
                        "mimeType": "text/plain",
                        "headers": [],
                        "body": {"size": 5, "data": body},
                    },
                    {
                        "partId": "att-1",
                        "mimeType": "application/pdf",
                        "filename": "offer.pdf",
                        "headers": [
                            {"name": "Content-Disposition", "value": "attachment; filename=offer.pdf"}
                        ],
                        "body": {"size": attachment_size, "attachmentId": "att-1"},
                    },
                ],
            }
        else:
            payload = {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": from_value},
                    {"name": "To", "value": EMAIL},
                    {"name": "Subject", "value": "Pull sample"},
                    {"name": "Message-ID", "value": "<message@example.test>"},
                ],
                "body": {"size": 5, "data": body},
            }
        dump(
            {
                "thread": {
                    "id": hex_id,
                    "historyId": "1",
                    "messages": [
                        {
                            "id": "1111111111111111",
                            "threadId": hex_id,
                            "labelIds": ["INBOX", "Label_1"],
                            "internalDate": "1793908800000",
                            "snippet": "PRIVATE_SENTINEL snippet" if scenario == "poison" else "hello",
                            "payload": payload,
                        }
                    ],
                }
            }
        )
    elif scenario in {"thread_malformed", "malformed_json"}:
        sys.stdout.write("{malformed JSON\n")
    else:
        dump({"thread": {"id": hex_id, "historyId": "1", "messages": []}})
    raise SystemExit(0)

if op == "message_get":
    hex_id = pos[2] if len(pos) > 2 else "msg1"
    if scenario == "message_mismatch":
        hex_id = "2222222222222222"
    if scenario in {"message_malformed", "malformed_json"}:
        sys.stdout.write("{malformed JSON\n")
        raise SystemExit(0)
    message = {"id": hex_id, "threadId": "0123456789abcdef", "labelIds": ["INBOX"]}
    if "metadata" not in args and scenario == "poison":
        message["snippet"] = "PRIVATE_SENTINEL message snippet"
        message["payload"] = {"mimeType": "text/plain", "body": {"data": "PRIVATE_SENTINEL"}}
    dump({"message": message})
    raise SystemExit(0)

if op == "url":
    hex_id = pos[2] if len(pos) > 2 else "1a0b9eb2cc26ab96"
    dump(
        {
            "urls": [
                f"https://mail.google.com/mail/?authuser={EMAIL}#all/{hex_id}"
            ]
        }
    )
    raise SystemExit(0)

if op == "attachment":
    # The cache-writing attachment command is forbidden in save tests: the
    # memory-only Discovery read above is the only attachment transport.
    fail(
        "fake gog: gmail attachment writes a cache file and is forbidden; "
        "use api call gmail.users.messages.attachments.get",
        2,
    )

if op == "labels_list":
    if scenario == "labels_failure":
        fail("PRIVATE_SENTINEL label lookup failure", 1)
    if scenario in {"labels_malformed", "malformed_json"}:
        sys.stdout.write("{malformed JSON\n")
    else:
        dump(
            {
                "labels": [
                    {"id": "Label_1", "name": "Promotions", "type": "user"},
                    {"id": "INBOX", "name": "INBOX", "type": "system"},
                ]
            }
        )
    raise SystemExit(0)

fail(f"fake gog: unknown command {pos}", 2)
