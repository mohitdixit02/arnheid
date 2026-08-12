"""Core Claude Agent SDK wrapper: one autonomous turn per call, with
per-chat session continuity and an isolated workspace directory per thread.

Chat transport (bot.py) never talks to the SDK directly — it just calls
run_turn(chat_key, text) and gets back the text to send.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from claude_agent_sdk import ClaudeAgentOptions, ResultMessage, query

WORKSPACE_ROOT = Path(os.environ.get("AGENT_WORKSPACE_DIR", "./workspace"))
SESSIONS_FILE = WORKSPACE_ROOT / "sessions.json"

ALLOWED_TOOLS = ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "WebSearch", "WebFetch"]
PERMISSION_MODE = os.environ.get("AGENT_PERMISSION_MODE", "bypassPermissions")

STANDING_INSTRUCTIONS = """You are a general-purpose autonomous engineering agent, talking to \
a human teammate through a chat app (Slack or Telegram). You have full shell, filesystem, and \
web access in this working directory, plus GitHub access via the `gh` CLI (GH_TOKEN is set in \
your environment — prefer `gh repo clone` / `gh pr create` over raw git with manual auth for \
GitHub repos, gh handles the credential wiring for you).

Work like a competent senior engineer operating independently:
- Do the work, don't just describe how you'd do it.
- When a task is genuinely ambiguous in a way that changes what you'd build, ask one crisp \
question instead of guessing — otherwise proceed.
- Report back concisely: what you did, links (PRs, files), and anything worth flagging (bugs \
found, risks, things you deliberately left out). No fluff, no step-by-step narration.
"""


@dataclass
class TurnResult:
    session_id: str
    text: str


class SessionStore:
    """chat_key -> {session_id, cwd}, persisted as one JSON file.

    ponytail: JSON file, not sqlite — single process, low write volume (one
    write per finished turn). Move to sqlite if this ever needs concurrent
    writers.
    """

    def __init__(self, path: Path = SESSIONS_FILE):
        self._path = path
        self._data: dict[str, dict] = {}
        if path.exists():
            self._data = json.loads(path.read_text())

    def get(self, chat_key: str) -> dict | None:
        return self._data.get(chat_key)

    def set(self, chat_key: str, session_id: str, cwd: str) -> None:
        self._data[chat_key] = {"session_id": session_id, "cwd": cwd}
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._path.write_text(json.dumps(self._data, indent=2))


sessions = SessionStore()


def _workspace_for(chat_key: str) -> Path:
    safe = chat_key.replace(":", "-").replace("/", "-")
    d = WORKSPACE_ROOT / safe
    d.mkdir(parents=True, exist_ok=True)
    return d


def _agent_env() -> dict:
    env = dict(os.environ)
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        env["GH_TOKEN"] = token
        env["GITHUB_TOKEN"] = token
    return env


async def run_turn(chat_key: str, user_text: str) -> TurnResult:
    """Run one autonomous agent turn for this chat thread, resuming its
    SDK session if one exists so follow-ups keep full context. Returns the
    final text to send back to the chat."""
    prior = sessions.get(chat_key)
    cwd = prior["cwd"] if prior else str(_workspace_for(chat_key))
    resume = prior["session_id"] if prior else None

    # Standing instructions only need to be said once — a resumed session
    # already has them in context.
    prompt = user_text if resume else f"{STANDING_INSTRUCTIONS}\nRequest: {user_text}"

    options = ClaudeAgentOptions(
        cwd=cwd,
        allowed_tools=ALLOWED_TOOLS,
        permission_mode=PERMISSION_MODE,
        resume=resume,
        env=_agent_env(),
    )

    session_id = resume
    result_text = ""
    async for message in query(prompt=prompt, options=options):
        if isinstance(message, ResultMessage):
            session_id = message.session_id
            result_text = message.result or "(no output)"

    if session_id:
        sessions.set(chat_key, session_id, cwd)
    return TurnResult(session_id=session_id or "", text=result_text)
