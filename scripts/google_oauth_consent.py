#!/usr/bin/env python3
"""One-time Google OAuth consent → the GOOGLE_REFRESH_TOKEN Arnheid needs.

    python3 scripts/google_oauth_consent.py ~/Downloads/client_secret_*.json

Opens a browser, asks for the scopes the built-in GSuite MCP backend uses
(`src/mcp/gsuite.rs`) — Gmail read + send, Drive read-only, Calendar read +
create event — and prints the three env vars to paste into .env.

Stdlib only, on purpose — no pip install to run one script once.

**Ask for every scope you will ever need, now.** The refresh_token grant returns
a token carrying the scopes the credential was ORIGINALLY consented for. Adding
one later means redoing consent, and until you do, the calls that need it fail
with a 403 that doesn't mention scopes. If you extend the GSuite tools to write
(send mail, create events), widen SCOPES here first and re-run.

Nothing is written to disk: output goes to stdout, and it contains a refresh
token — treat it exactly like a password.
"""

from __future__ import annotations

import http.server
import json
import sys
import urllib.parse
import urllib.request
import webbrowser

# Matches what src/mcp/gsuite.rs actually calls — no more, no less. gmail.send
# is additive to gmail.readonly (it does not grant delete/modify); Drive stays
# readonly (this credential should never be able to move/delete files);
# calendar.events is the narrowest scope that allows creating events
# (calendar_create_event) without also granting calendar/ACL management.
SCOPES = [
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.send",
    "https://www.googleapis.com/auth/calendar.events",
    "https://www.googleapis.com/auth/drive.readonly",
]
AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth"
TOKEN_URL = "https://oauth2.googleapis.com/token"


class _Catch(http.server.BaseHTTPRequestHandler):
    """Single-shot loopback receiver for the ?code= redirect."""

    code: str | None = None

    def do_GET(self) -> None:  # noqa: N802 - stdlib naming
        params = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        _Catch.code = (params.get("code") or [None])[0]
        error = (params.get("error") or [None])[0]
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        body = f"Google said: {error}" if error else "Consent captured. Close this tab."
        self.wfile.write(body.encode())

    def log_message(self, *_args: object) -> None:
        pass  # the HTTP log is noise around the one line that matters


def main(client_secret_path: str) -> int:
    with open(client_secret_path) as fh:
        blob = json.load(fh)
    creds = blob.get("installed") or blob.get("web")
    if not creds:
        print("Not an OAuth client file: expected an 'installed' or 'web' key.", file=sys.stderr)
        return 2
    client_id, client_secret = creds["client_id"], creds["client_secret"]

    # Port 0 lets the OS pick; Google accepts any port on http://localhost for
    # an installed-app client, so nothing needs registering up front.
    server = http.server.HTTPServer(("localhost", 0), _Catch)
    redirect_uri = f"http://localhost:{server.server_port}"

    consent = AUTH_URL + "?" + urllib.parse.urlencode(
        {
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "response_type": "code",
            "scope": " ".join(SCOPES),
            # access_type=offline is what makes Google return a refresh_token at
            # all; prompt=consent forces a NEW one even if this account already
            # consented, which is the difference between a credential that keeps
            # working and one that dies when the access token expires in an hour.
            "access_type": "offline",
            "prompt": "consent",
        }
    )
    print(f"Opening browser for consent. If nothing opens, visit:\n{consent}\n", file=sys.stderr)
    webbrowser.open(consent)
    server.handle_request()
    if not _Catch.code:
        print("No authorization code received.", file=sys.stderr)
        return 1

    request = urllib.request.Request(
        TOKEN_URL,
        data=urllib.parse.urlencode(
            {
                "code": _Catch.code,
                "client_id": client_id,
                "client_secret": client_secret,
                "redirect_uri": redirect_uri,
                "grant_type": "authorization_code",
            }
        ).encode(),
    )
    with urllib.request.urlopen(request) as response:
        token = json.load(response)

    if not token.get("refresh_token"):
        print(
            "Google returned no refresh_token. Revoke this app at "
            "https://myaccount.google.com/permissions and run again.",
            file=sys.stderr,
        )
        return 1

    print(f"GOOGLE_CLIENT_ID={client_id}")
    print(f"GOOGLE_CLIENT_SECRET={client_secret}")
    print(f"GOOGLE_REFRESH_TOKEN={token['refresh_token']}")
    print(f"\nConsented scopes: {token.get('scope', '')}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(main(sys.argv[1]))
