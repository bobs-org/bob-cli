#!/usr/bin/env python3
"""Fake gog for bob gcal tests. Records argv; never talks to Google."""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

CLIENT = "bob-gcal"
SCOPE = "https://www.googleapis.com/auth/calendar.readonly"
EMAIL = "you@example.com"

FLAGS_WITH_VALUE = {
    "--color",
    "--client",
    "--account",
    "--services",
    "--from",
    "--to",
    "--max",
    "--fields",
    "--format",
    "--auth-url",
    "--step",
    "--redirect-uri",
    "--timeout",
    "--listen-addr",
    "--redirect-host",
}

args = sys.argv[1:]
log_path = os.environ.get("BOB_GCAL_FAKE_LOG")
state_path = os.environ.get("BOB_GCAL_FAKE_STATE")
scenario = os.environ.get("BOB_GCAL_FAKE_SCENARIO", "ok")
fake_home = Path(os.environ.get("BOB_GCAL_FAKE_HOME", "/tmp"))
stdin_log = os.environ.get("BOB_GCAL_FAKE_STDIN_LOG")

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
    if name in args:
        index = args.index(name)
        if index + 1 < len(args):
            return args[index + 1]
    return None


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


def default_calendars() -> list[dict]:
    return [
        {
            "id": EMAIL,
            "summary": "Personal",
            "primary": True,
            "accessRole": "owner",
            "backgroundColor": "#9fc6e7",
            "colorId": "1",
        },
        {
            "id": "work@group.calendar.google.com",
            "summary": "Work",
            "primary": False,
            "accessRole": "reader",
            "backgroundColor": "#fad165",
            "colorId": "5",
        },
    ]


def connected_account(email: str = EMAIL) -> dict:
    return {
        "email": email,
        "client": CLIENT,
        "services": ["calendar"],
        "scopes": [SCOPE],
    }


def legacy_flat_client(client_id: str, secret: str) -> dict:
    return {"client_id": client_id, "client_secret": secret}


def scenario_defaults(name: str) -> dict:
    home = fake_home
    try:
        home.mkdir(parents=True, exist_ok=True)
    except Exception:
        pass
    bob_path = str(home / "bob-gcal.json")
    defaults: dict = {
        "version": "0.44.0",
        "clients": [
            {"client": CLIENT, "path": bob_path, "default": True}
        ],
        "accounts": [connected_account()],
        "calendars": default_calendars(),
    }
    if name == "too_old":
        defaults["version"] = "0.43.0"
    elif name == "no_client":
        defaults["clients"] = []
        defaults["accounts"] = []
    elif name == "other_client":
        other_path = home / "default-client.json"
        other_path.write_text(
            json.dumps(
                legacy_flat_client(
                    "123456789012-abc.apps.googleusercontent.com",
                    "other-secret",
                )
            ),
            encoding="utf-8",
        )
        defaults["clients"] = [
            {
                "client": "default",
                "path": str(other_path),
                "default": True,
            }
        ]
        defaults["accounts"] = [
            {
                "email": EMAIL,
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            }
        ]
    elif name == "keyring_client":
        keyring_path = home / "keyring-client.json"
        keyring_path.write_text(
            json.dumps({"client_id": "123456789012-abc.apps.googleusercontent.com"}),
            encoding="utf-8",
        )
        defaults["clients"] = [
            {
                "client": "default",
                "path": str(keyring_path),
                "default": True,
                "client_secret_in_keyring": True,
            }
        ]
        defaults["accounts"] = [
            {
                "email": EMAIL,
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            }
        ]
    elif name == "keyring_fail":
        defaults["clients"] = []
        defaults["accounts"] = []
        defaults["fail"] = {
            "credentials_set": {
                "stderr": "cannot store secret: keyring is locked",
                "code": 1,
            }
        }
    elif name == "not_logged_in":
        defaults["accounts"] = []
    elif name == "no_scope":
        defaults["accounts"] = [
            {
                "email": EMAIL,
                "client": CLIENT,
                "services": [],
                "scopes": [
                    "https://www.googleapis.com/auth/gmail.readonly"
                ],
            }
        ]
    elif name == "needs_auth":
        defaults["accounts"] = []
    elif name == "api_disabled":
        defaults["fail"] = {
            "calendars": {
                "stderr": (
                    "accessNotConfigured: Google Calendar API has not "
                    "been used in project 123456789012"
                ),
                "code": 1,
            }
        }
    elif name == "api_disabled_once":
        pass
    elif name == "stale_grant":
        pass
    elif name == "two_default_accounts":
        defaults["accounts"] = [
            connected_account(),
            {
                "email": "work@example.com",
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            },
            {
                "email": "other@example.com",
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            },
        ]
    elif name == "picker_accounts":
        defaults["accounts"] = [
            {
                "email": EMAIL,
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            },
            {
                "email": "work@example.com",
                "client": "default",
                "services": ["gmail"],
                "scopes": [],
            },
        ]
    elif name == "two_bob_accounts":
        defaults["accounts"] = [
            connected_account(),
            {
                "email": "work@example.com",
                "client": CLIENT,
                "services": ["gmail"],
                "scopes": [],
            },
        ]
    elif name == "stored_project":
        Path(bob_path).write_text(
            json.dumps(
                {
                    "installed": {
                        "client_id": "123456789012-abc.apps.googleusercontent.com",
                        "client_secret": "fake-secret",
                        "project_id": "doctor-project",
                    }
                }
            ),
            encoding="utf-8",
        )
    elif name == "invalid_grant":
        defaults["fail"] = {
            "calendars": {
                "stderr": "invalid_grant: Token has been expired or revoked",
                "code": 1,
            }
        }
    elif name == "insufficient_scope":
        defaults["fail"] = {
            "calendars": {
                "stderr": "ACCESS_TOKEN_SCOPE_INSUFFICIENT",
                "code": 1,
            }
        }
    elif name == "network":
        defaults["fail"] = {
            "calendars": {
                "stderr": "dial tcp: connection refused",
                "code": 1,
            }
        }
    elif name == "events_failure":
        defaults["fail"] = {
            "events": {
                "stderr": "fake gog: second page failed",
                "code": 1,
            }
        }
    elif name == "sleep":
        defaults["sleep"] = 2
    elif name == "exchange_invalid_grant":
        defaults["accounts"] = []
        defaults["fail"] = {
            "auth_finish": {
                "stderr": "invalid_grant: Token has been expired or revoked",
                "code": 1,
            }
        }
    elif name == "exchange_invalid_client":
        defaults["accounts"] = []
        defaults["fail"] = {
            "auth_finish": {
                "stderr": "invalid_client: The OAuth client was not found.",
                "code": 1,
            }
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

sleep_for = state.get("sleep")
if sleep_for:
    time.sleep(float(sleep_for))

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
    if pos[:2] == ["calendar", "calendars"]:
        return "calendars"
    if pos[:2] == ["calendar", "events"]:
        return "events"
    if pos[:2] == ["calendar", "event"]:
        return "event"
    return "unknown"


op = op_name()
fail_spec = (state.get("fail") or {}).get(op)
if fail_spec:
    fail(fail_spec.get("stderr") or "gog failed", int(fail_spec.get("code") or 1))

if scenario == "api_disabled_once" and op == "calendars":
    if not state.get("api_disabled_once_seen"):
        state["api_disabled_once_seen"] = True
        save_state(state)
        fail(
            "accessNotConfigured: Google Calendar API has not been used in project 123456789012",
            1,
        )

if scenario == "stale_grant" and op == "calendars":
    if not state.get("grant_repaired"):
        fail("invalid_grant: Token has been expired or revoked", 1)

if op == "version":
    dump(
        {
            "version": state.get("version", "0.44.0"),
            "commit": "test",
            "date": "2026-10-30",
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
        stored_path = str(fake_home / "bob-gcal.json")
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

if op == "auth_add":
    if "--remote" not in args:
        fail("fake gog: interactive auth add is not supported", 2)
    step = flag_value("--step") or ""
    email = pos[2] if len(pos) > 2 else EMAIL
    if step == "1":
        redirect = flag_value("--redirect-uri") or (
            "http://127.0.0.1:36015/oauth2/callback"
        )
        from urllib.parse import quote

        auth_url = (
            "https://accounts.google.com/o/oauth2/auth"
            "?access_type=offline"
            "&client_id=123456789012-abc.apps.googleusercontent.com"
            f"&redirect_uri={quote(redirect, safe='')}"
            "&response_type=code"
            "&state=fakestate"
        )
        state["manual_state"] = "fakestate"
        save_state(state)
        dump({"auth_url": auth_url, "state_reused": False})
        raise SystemExit(0)
    if step == "2":
        finish_fail = (state.get("fail") or {}).get("auth_finish")
        if finish_fail:
            fail(
                finish_fail.get("stderr") or "gog failed",
                int(finish_fail.get("code") or 1),
            )
        from urllib.parse import parse_qs, urlparse

        auth_url = flag_value("--auth-url") or ""
        query = parse_qs(urlparse(auth_url).query)
        got_state = (query.get("state") or [None])[0]
        got_code = (query.get("code") or [None])[0]
        if not got_state:
            fail("missing state in redirect URL")
        expected = state.get("manual_state")
        if got_state != expected:
            fail(
                "manual auth state missing; start a new manual flow or "
                "run remote step 1 again"
            )
        if not got_code:
            fail("no code found in URL")
        accounts = [
            entry
            for entry in (state.get("accounts") or [])
            if not (
                entry.get("email") == email and entry.get("client") == CLIENT
            )
        ]
        accounts.append(connected_account(email))
        state["accounts"] = accounts
        if scenario == "stale_grant":
            state["grant_repaired"] = True
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

if op == "calendars":
    dump(
        {
            "calendars": state.get("calendars") or [],
            "nextPageToken": None,
        }
    )
    raise SystemExit(0)

if op == "events":
    fixture = os.environ.get("BOB_GCAL_FAKE_EVENTS")
    if fixture and Path(fixture).exists():
        sys.stdout.write(Path(fixture).read_text(encoding="utf-8"))
        if not Path(fixture).read_text(encoding="utf-8").endswith("\n"):
            sys.stdout.write("\n")
    else:
        dump({"events": [], "nextPageToken": None})
    raise SystemExit(0)

if op == "event":
    event_id = pos[3] if len(pos) > 3 else (pos[2] if len(pos) > 2 else "event")
    event_dir = os.environ.get("BOB_GCAL_FAKE_EVENT_DIR")
    if event_dir:
        candidate = Path(event_dir) / f"{event_id}.json"
        if candidate.exists():
            payload = json.loads(candidate.read_text(encoding="utf-8"))
            failure = payload.get("_fail")
            if failure is not None:
                fail(
                    failure.get("stderr") or "gog failed",
                    int(failure.get("code") or 1),
                )
            dump(payload)
            raise SystemExit(0)
        fail(f"Not Found: no such event {event_id}", 1)
    dump({"event": {"id": event_id}})
    raise SystemExit(0)

fail(f"fake gog: unknown command {pos}", 2)
