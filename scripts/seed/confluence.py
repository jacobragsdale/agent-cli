"""Set up scripts/confluence-up.sh's Confluence Data Center and seed it for
crates/cli/tests/live_confluence.rs: walk the setup wizard (one node, an
empty site, users kept in Confluence, an admin), add Jane Doe, Sam Lee and
Priya Patel, make Jane a personal access token and her spaces ENG and OPS.

The token goes to target/confluence-live/token (0600). Every password here
is throwaway; the site listens on 127.0.0.1 only.

    python3 -I scripts/seed/confluence.py BASE_URL"""

import base64
import http.cookiejar
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

PASSWORD = "agent-cli-pass"
PEOPLE = [("jane", "Jane Doe"), ("sam", "Sam Lee"), ("priya", "Priya Patel")]
SPACES = [("ENG", "Engineering"), ("OPS", "Operations")]


def basic(user):
    return "Basic " + base64.b64encode(f"{user}:{PASSWORD}".encode()).decode()


class Site:
    def __init__(self, base):
        self.base = base
        jar = http.cookiejar.CookieJar()
        self.browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

    def status(self):
        try:
            with urllib.request.urlopen(f"{self.base}/status", timeout=10) as answer:
                return json.load(answer).get("state")
        except (urllib.error.URLError, OSError, ValueError):
            return None

    def wait(self, *states, minutes=10):
        deadline = time.time() + minutes * 60
        while (state := self.status()) not in states:
            if time.time() > deadline:
                sys.exit(f"confluence: still {state} after {minutes} minutes, wanted {states}")
            time.sleep(5)
        return state

    def step(self, page, action, fields):
        """One wizard form, as a browser fills it: read its XSRF token from
        the page, post it once and follow where it sends, which is where the
        site does its setup's work; answer where that ends. Sending a form
        twice runs that work twice at once, and the site comes up without
        some of its plugins."""
        with self.browser.open(f"{self.base}/setup/{page}", timeout=120) as answer:
            html = answer.read().decode()
        token = re.search(r'name="atl_token" value="([^"]+)"', html)
        data = urllib.parse.urlencode(dict(fields, atl_token=token.group(1) if token else ""))
        with self.browser.open(f"{self.base}/setup/{action}", data=data.encode(), timeout=900) as answer:
            return answer.geturl()

    def settle(self):
        """Opens the site as a browser does, which the bootstrap sends to the
        wizard's first step. That answers 500 for a while after the status
        says FIRST_RUN, and its forms fail for a while after it answers."""
        deadline = time.time() + 600
        while True:
            try:
                with self.browser.open(f"{self.base}/", timeout=120):
                    break
            except urllib.error.HTTPError as error:
                if error.code < 500 or time.time() > deadline:
                    raise
                time.sleep(5)
        time.sleep(60)

    def call(self, method, path, body=None, auth=None, ok=(200, 201, 204)):
        request = urllib.request.Request(f"{self.base}{path}", method=method)
        request.add_header("Accept", "application/json")
        request.add_header("Authorization", auth or basic("admin"))
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, data=data, timeout=120) as answer:
                text = answer.read().decode()
                return answer.status, json.loads(text) if text.strip() else None
        except urllib.error.HTTPError as error:
            if error.code in ok:
                return error.code, None
            return error.code, error.read().decode()


def setup(site):
    """The wizard's four steps, once: the licence and the database come
    from the container's ATL_ variables."""
    site.settle()
    site.step("setupcluster-start.action", "setupcluster.action", {"newCluster": "skipCluster"})
    site.step("setupdata-start.action", "setupdata.action",
              {"contentChoice": "blank", "dbchoiceSelect": "Empty Site"})
    site.step("setupusermanagementchoice-start.action", "setupusermanagementchoice.action",
              {"userManagementChoice": "internal", "internal": "Manage users and groups within Confluence"})
    done = site.step("setupadministrator-start.action", "setupadministrator.action",
                     {"username": "admin", "fullName": "Site Admin", "email": "admin@contoso.com",
                      "password": PASSWORD, "confirm": PASSWORD, "setup-next-button": "Next"})
    if not done.endswith("finishsetup.action"):
        sys.exit(f"confluence: the setup wizard ended at {done}")


def main():
    base = sys.argv[1].rstrip("/")
    site = Site(base)
    if site.wait("FIRST_RUN", "RUNNING") == "FIRST_RUN":
        setup(site)
        site.wait("RUNNING")
    for username, name in PEOPLE:
        status, said = site.call("POST", "/rest/api/admin/user", {
            "userName": username, "fullName": name, "email": f"{username}@contoso.com",
            "password": PASSWORD, "notifyViaEmail": False})
        if status not in (200, 201) and "already exists" not in str(said):
            sys.exit(f"confluence: user {username}: {status} {said}")
    # The tokens plugin starts minutes after the wizard ends: 404, then 500.
    deadline = time.time() + 600
    while True:
        status, token = site.call("POST", "/rest/pat/latest/tokens",
                                  {"name": f"agent-cli-{int(time.time())}", "expirationDuration": 90},
                                  auth=basic("jane"))
        if status in (200, 201) or time.time() > deadline:
            break
        time.sleep(10)
    if status not in (200, 201) or not isinstance(token, dict):
        sys.exit(f"confluence: jane's token: {status} {token}")
    raw = token["rawToken"]
    bearer = f"Bearer {raw}"
    for key, name in SPACES:
        status, said = site.call("POST", "/rest/api/space", {"key": key, "name": name}, auth=bearer)
        if status != 200 and "already exists" not in str(said):
            sys.exit(f"confluence: space {key}: {status} {said}")
    out = os.path.join("target", "confluence-live")
    os.makedirs(out, exist_ok=True)
    path = os.path.join(out, "token")
    with open(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as file:
        file.write(raw)
    print(f"confluence: {base}, Jane Doe's token in {path}")


if __name__ == "__main__":
    main()
