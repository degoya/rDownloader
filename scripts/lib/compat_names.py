"""The named contracts behind scripts/lib/compat-check.py (RD-1480-10, API-14): what a client
holds on to that neither the OpenAPI document nor the WIT says.

  rest:code-removed:<code>                 an error or message code a client tests for
  events:event-removed:<name>              an event name of the SSE stream
  webhook:field-removed:<message>.<field>  a field of the JSON a webhook receives
  webhook:header-removed:<header>          a header it is sent with
  mcp:tool-removed:<tool>                  a tool of the MCP server

Each list is read from the sources that already hold it, at both refs, instead of from a generated
file: a file that does not exist at the base could not be compared with it, and these places are
held to the running service by tests of their own.

* Codes: the `codes` of the English server catalogue, the texts the web interface has for them. A
  code whose text left while a crate still writes it is no removal.
* Events: `event_name` and the stream's own markers in rd-api's event_stream.rs.
* Webhook fields: the keys of the functions that build the body; headers: delivery.rs.
* Tools: TOOL_POLICY, which rd-api's tests hold to the tools the server lists.

Called as a script it prints the paths it reads, for scripts/compat-check.sh to fetch them at the
base."""

import json
import pathlib
import re
import sys

CODES = "web/src/locales/en/server.json"
EVENTS = "crates/rd-api/src/event_stream.rs"
TOOLS = "crates/rd-api-mcp/src/policy.rs"
WEBHOOK_HEADERS = "crates/rd-notify/src/delivery.rs"
# The functions whose JSON a webhook receives: {message: [(file, function)]}. The last one fills
# in what both messages carry.
WEBHOOK_SUBJECT = ("crates/rd-notify/src/subject.rs", "describe")
WEBHOOK_BODIES = {
    "notification": [("crates/rd-notify/src/summary.rs", "message_of"), WEBHOOK_SUBJECT],
    "summary": [("crates/rd-notify/src/summary.rs", "quiet_summary"), WEBHOOK_SUBJECT],
    "automation": [
        ("crates/rd-api-core/src/automation_actions.rs", "automation_message"), WEBHOOK_SUBJECT],
}

PATHS = sorted({CODES, EVENTS, TOOLS, WEBHOOK_HEADERS}
               | {path for parts in WEBHOOK_BODIES.values() for path, _ in parts})


class Unreadable(Exception):
    """A list the base has and the working tree's sources no longer yield."""


def read(tree, path):
    try:
        return (tree / path).read_text(encoding="utf-8")
    except OSError:
        return ""


def body_of(source, function):
    """The text of one Rust function, up to the line its body closes on."""
    found = re.search(rf"^( *)(?:pub(?:\([a-z]+\))? )?fn {function}\(.*?^\1}}$", source,
                      re.S | re.M)
    return found.group(0) if found else ""


def codes(tree):
    try:
        return set(json.loads(read(tree, CODES) or "{}").get("codes", {}))
    except json.JSONDecodeError as error:
        raise Unreadable(f"{CODES}: {error}") from error


def events(tree):
    source = read(tree, EVENTS)
    return set(re.findall(r'EventKind::\w+\s*=>\s*"([^"]+)"', body_of(source, "event_name"))) | set(
        re.findall(r'^(?:pub(?:\([a-z]+\))? )?const \w+_EVENT: &str = "([^"]+)";', source, re.M))


def tools(tree):
    return set(re.findall(r'\btool\(\s*"(\w+)"', read(tree, TOOLS)))


def webhook_headers(tree):
    return set(re.findall(r'^pub const \w+_HEADER: &str = "([^"]+)";', read(tree, WEBHOOK_HEADERS),
                          re.M))


def webhook_fields(tree):
    fields = set()
    for message, parts in WEBHOOK_BODIES.items():
        for path, function in parts:
            body = body_of(read(tree, path), function)
            keys = re.findall(r'^\s*"(\w+)":', body, re.M)
            keys += re.findall(r'\b(?:payload|body)\["(\w+)"\]', body)
            fields |= {f"{message}.{key}" for key in keys}
    return fields


LISTS = (
    ("codes", codes, "rest:code-removed", "the error code is gone"),
    ("events", events, "events:event-removed", "the event name is gone"),
    ("webhook fields", webhook_fields, "webhook:field-removed",
     "the body of a webhook no longer carries this field"),
    ("webhook headers", webhook_headers, "webhook:header-removed",
     "a webhook is no longer sent with this header"),
    ("MCP tools", tools, "mcp:tool-removed", "the tool is gone"),
)


def still_written(tree, wanted):
    """Those of `wanted` that a crate's sources still hold as a string."""
    found = set()
    for path in sorted((tree / "crates").glob("*/src/**/*.rs")):
        if not wanted - found:
            break
        source = path.read_text(encoding="utf-8", errors="replace")
        found |= {name for name in wanted - found if f'"{name}"' in source}
    return found


def compare_names(old_tree, new_tree):
    """(findings, notes) between two checkouts, or Unreadable."""
    old_tree, new_tree = pathlib.Path(old_tree), pathlib.Path(new_tree)
    findings, notes = [], []
    for name, read_list, kind, message in LISTS:
        before = read_list(old_tree)
        if not before:
            notes.append(f"note: the base names no {name}; they are not compared")
            continue
        now = read_list(new_tree)
        if not now:
            raise Unreadable(f"the working tree names no {name} where compat_names.py reads them; "
                             "point it at their new place")
        lost = before - now
        if read_list is codes and lost:
            lost -= still_written(new_tree, lost)
        findings += [(f"{kind}:{key}", message) for key in sorted(lost)]
    return findings, notes


if __name__ == "__main__":
    print("\n".join(PATHS))
    sys.exit(0)
