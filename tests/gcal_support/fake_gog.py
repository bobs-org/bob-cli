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
}

args = sys.argv[1:]
log_path = os.environ.get("BOB_GCAL_FAKE_LOG")
state_path = os.environ.get("BOB_GCAL_FAKE_STATE")
scenario = os.environ.get("BOB_GCAL_FAKE_SCENARIO", "ok")

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


def scenario_defaults(name: str) -> dict:
    defaults: dict = {
        "version": "0.44.0",
        "clients": [
            {"client": CLIENT, "path": "/tmp/bob-gcal.json", "default": True}
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
        defaults["clients"] = [
            {
                "client": "default",
                "path": "/tmp/default-client.json",
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
                    "been used in project"
                ),
                "code": 1,
            }
        }
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
    return defaults


def dump(payload: object) -> None:
    sys.stdout.write(json.dumps(payload) + "\n")


def fail(message: str, code: int = 1) -> None:
    sys.stderr.write(message + "\n")
    raise SystemExit(code)


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
    email = pos[2] if len(pos) > 2 else EMAIL
    accounts = [
        entry
        for entry in (state.get("accounts") or [])
        if not (entry.get("email") == email and entry.get("client") == CLIENT)
    ]
    accounts.append(connected_account(email))
    state["accounts"] = accounts
    save_state(state)
    raise SystemExit(0)

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
