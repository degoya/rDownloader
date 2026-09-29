#!/usr/bin/env python3
"""The breaking-change detector behind scripts/compat-check.sh (RD-170-08).

Compares two versions of the public contracts — the REST API (`web/openapi.json`) and the plugin
contract (`crates/rd-plugin-api/wit/rdownloader.wit`) — and prints one line per break. A break
passes when scripts/compat-breaks.toml acknowledges it for a release after the base, or, for the
WIT, when the package version moved far enough (a major bump; before 1.0, a minor one).

Every finding is `<area>:<kind>:<location>`, and that exact string is what an acknowledgement
names. Additions are never findings, except the ones the component model does not treat as
additions (docs/plugins.md#compatibility): a new record field, variant case, enum case or flag,
a new function on an interface a world exports, a new export of an existing world.

  compat-check.py --base-version 1.5.2 [--acks FILE]
                  [--old-openapi A --new-openapi B] [--old-wit C --new-wit D]

Exit 0 when every break is acknowledged, 1 when one is not, 2 on unusable input.
"""

import argparse
import json
import re
import sys
import tomllib

# --- REST ---------------------------------------------------------------------------------------

METHODS = ("get", "put", "post", "delete", "patch", "head", "options", "trace")


def ref_name(schema):
    ref = schema.get("$ref") if isinstance(schema, dict) else None
    return ref.rsplit("/", 1)[-1] if ref else None


def unwrap(schema):
    """(nullable, schema without its null alternative). utoipa writes Option<T> both ways."""
    if not isinstance(schema, dict):
        return False, {}
    types = schema.get("type")
    if isinstance(types, list) and "null" in types:
        rest = [t for t in types if t != "null"]
        return True, {**schema, "type": rest[0] if len(rest) == 1 else rest}
    for key in ("oneOf", "anyOf"):
        members = schema.get(key)
        if isinstance(members, list) and {"type": "null"} in members:
            rest = [m for m in members if m != {"type": "null"}]
            return True, rest[0] if len(rest) == 1 else {**schema, key: rest}
    for key in ("allOf", "oneOf", "anyOf"):
        if isinstance(schema.get(key), list) and len(schema[key]) == 1:
            return unwrap(schema[key][0])
    return False, schema


def type_set(schema):
    types = schema.get("type")
    if types is None:
        return set()
    return {t for t in ([types] if isinstance(types, str) else types) if t != "null"}


def enum_values(schema):
    """The closed set of values a schema allows, or None when it is not an enumeration."""
    if isinstance(schema.get("enum"), list):
        return {json.dumps(v) for v in schema["enum"]}
    members = schema.get("oneOf") or schema.get("anyOf")
    if isinstance(members, list) and members:
        values = set()
        for member in members:
            if isinstance(member.get("enum"), list) and len(member["enum"]) == 1:
                values.add(json.dumps(member["enum"][0]))
            elif "const" in member:
                values.add(json.dumps(member["const"]))
            else:
                return None
        return values
    return None


def variant_key(index, member):
    """How a oneOf member is recognised across versions: its $ref, or its tag property."""
    name = ref_name(member)
    if name:
        return name
    tags = sorted(
        f"{prop}={prop_schema['enum'][0]}"
        for prop, prop_schema in (member.get("properties") or {}).items()
        if isinstance(prop_schema, dict)
        and isinstance(prop_schema.get("enum"), list)
        and len(prop_schema["enum"]) == 1
    )
    return ",".join(tags) if tags else f"#{index}"


class Rest:
    def __init__(self, old, new):
        self.old, self.new = old, new
        self.findings = []
        self.seen_pairs = set()

    def schema(self, spec, name):
        return (spec.get("components", {}).get("schemas", {}) or {}).get(name, {})

    def add(self, kind, location, message):
        self.findings.append((f"rest:{kind}:{location}", message))

    def flatten(self, spec, schema, depth=0):
        """An allOf composition as the one object it describes: properties and required merged."""
        if not isinstance(schema.get("allOf"), list) or depth > 8:
            return schema
        merged = {"type": "object", "properties": {}, "required": []}
        for member in schema["allOf"]:
            name = ref_name(member)
            member = self.flatten(spec, unwrap(self.schema(spec, name) if name else member)[1],
                                  depth + 1)
            merged["properties"].update(member.get("properties") or {})
            merged["required"] += member.get("required") or []
        return merged

    def compare(self, old, new, dirs, loc):
        """Old and new schema at one place, used for requests, responses or both (`dirs`)."""
        old_ref, new_ref = ref_name(old), ref_name(new)
        if old_ref and old_ref == new_ref:
            return  # compared once, as a component
        if old_ref or new_ref:
            pair = (old_ref, new_ref, loc)
            if pair in self.seen_pairs:
                return
            self.seen_pairs.add(pair)
            # A renamed DTO with the same shape is no break; its content is what counts.
            old = self.schema(self.old, old_ref) if old_ref else old
            new = self.schema(self.new, new_ref) if new_ref else new
        old_null, old = unwrap(old)
        new_null, new = unwrap(new)
        if "response" in dirs and new_null and not old_null:
            self.add("response-nullable", loc, "a response value may now be null")
        if "request" in dirs and old_null and not new_null:
            self.add("request-not-nullable", loc, "a request value may no longer be null")
        if ref_name(old) or ref_name(new):
            return self.compare(old, new, dirs, loc)
        old, new = self.flatten(self.old, old), self.flatten(self.new, new)
        old_types, new_types = type_set(old), type_set(new)
        if old_types and new_types and old_types != new_types:
            self.add("type-changed", loc,
                     f"type {'/'.join(sorted(old_types))} -> {'/'.join(sorted(new_types))}")
            return
        old_enum, new_enum = enum_values(old), enum_values(new)
        if old_enum is not None and new_enum is not None:
            lost = old_enum - new_enum
            if lost:
                self.add("enum-narrowed", loc, f"values removed: {', '.join(sorted(lost))}")
            return
        self.compare_object(old, new, dirs, loc)
        if isinstance(old.get("items"), dict) and isinstance(new.get("items"), dict):
            self.compare(old["items"], new["items"], dirs, f"{loc}[]")
        if isinstance(old.get("additionalProperties"), dict) and isinstance(
                new.get("additionalProperties"), dict):
            self.compare(old["additionalProperties"], new["additionalProperties"], dirs,
                         f"{loc}{{}}")
        for key in ("oneOf", "anyOf"):
            if isinstance(old.get(key), list) and isinstance(new.get(key), list):
                self.compare_variants(old[key], new[key], dirs, loc)

    def compare_object(self, old, new, dirs, loc):
        old_props, new_props = old.get("properties") or {}, new.get("properties") or {}
        old_req, new_req = set(old.get("required") or []), set(new.get("required") or [])
        for prop, schema in old_props.items():
            if prop not in new_props:
                if "response" in dirs:
                    self.add("response-property-removed", f"{loc}.{prop}", "property removed")
                continue
            self.compare(schema, new_props[prop], dirs, f"{loc}.{prop}")
        if "request" in dirs:
            for prop in sorted(new_req - old_req):
                if prop in new_props:
                    self.add("request-property-required", f"{loc}.{prop}",
                             "a request must now carry this property")
        if "response" in dirs:
            for prop in sorted((old_req - new_req) & set(new_props)):
                self.add("response-property-optional", f"{loc}.{prop}",
                         "a response may now leave this property out")

    def compare_variants(self, old, new, dirs, loc):
        old_keys = {variant_key(i, m): m for i, m in enumerate(old)}
        new_keys = {variant_key(i, m): m for i, m in enumerate(new)}
        for key, member in old_keys.items():
            if key not in new_keys:
                self.add("variant-removed", f"{loc}<{key}>", "a oneOf alternative was removed")
            else:
                self.compare(member, new_keys[key], dirs, f"{loc}<{key}>")

    # --- operations -----------------------------------------------------------------------------

    @staticmethod
    def operations(spec):
        """{(method, normalised path): (display path, operation)}; `{id}` and `{job_id}` match."""
        found = {}
        for path, item in (spec.get("paths") or {}).items():
            for method, operation in item.items():
                if method in METHODS and isinstance(operation, dict):
                    found[(method, re.sub(r"\{[^}]*\}", "{}", path))] = (path, operation)
        return found

    @staticmethod
    def usage(spec):
        """{component name: {"request", "response"}} over everything an operation reaches."""
        schemas = (spec.get("components", {}) or {}).get("schemas", {}) or {}
        dirs = {}

        def walk(node, direction):
            if isinstance(node, dict):
                name = ref_name(node)
                if name:
                    if direction in dirs.setdefault(name, set()):
                        return
                    dirs[name].add(direction)
                    walk(schemas.get(name, {}), direction)
                for value in node.values():
                    walk(value, direction)
            elif isinstance(node, list):
                for value in node:
                    walk(value, direction)

        for _, operation in Rest.operations(spec).values():
            walk(operation.get("parameters"), "request")
            walk(operation.get("requestBody"), "request")
            walk(operation.get("responses"), "response")
        return dirs

    def run(self):
        old_ops, new_ops = self.operations(self.old), self.operations(self.new)
        new_paths = {key[1] for key in new_ops}
        reported_paths = set()
        for key, (path, old_op) in sorted(old_ops.items()):
            method = key[0].upper()
            if key not in new_ops:
                if key[1] not in new_paths:
                    if key[1] not in reported_paths:
                        reported_paths.add(key[1])
                        self.add("path-removed", path, "path removed")
                else:
                    self.add("operation-removed", f"{method} {path}", "method removed")
                continue
            self.compare_operation(f"{method} {path}", old_op, new_ops[key][1])

        old_use, new_use = self.usage(self.old), self.usage(self.new)
        old_schemas = (self.old.get("components", {}) or {}).get("schemas", {}) or {}
        for name in sorted(old_schemas):
            dirs = old_use.get(name, set()) | new_use.get(name, set())
            if dirs and name in ((self.new.get("components", {}) or {}).get("schemas", {}) or {}):
                self.compare(old_schemas[name], self.schema(self.new, name), dirs,
                             f"schemas.{name}")
        return self.findings

    def compare_operation(self, where, old, new):
        def params(operation):
            return {(p.get("in"), p.get("name")): p for p in operation.get("parameters") or []
                    if isinstance(p, dict)}

        old_params, new_params = params(old), params(new)
        for (place, name), param in sorted(new_params.items()):
            before = old_params.get((place, name))
            # A path parameter is part of the path; renaming `{id}` changes no request.
            if place != "path" and param.get("required") and not (
                    before and before.get("required")):
                self.add("parameter-required", f"{where} {place}.{name}",
                         "a request must now carry this parameter")
            if before is not None:
                self.compare(before.get("schema", {}), param.get("schema", {}), {"request"},
                             f"{where} {place}.{name}")

        old_body, new_body = old.get("requestBody") or {}, new.get("requestBody") or {}
        if new_body.get("required") and not (old_body and old_body.get("required")):
            self.add("request-body-required", where, "a request body is now required")
        old_content, new_content = old_body.get("content") or {}, new_body.get("content") or {}
        for media, body in sorted(old_content.items()):
            if media not in new_content:
                if new_content:
                    self.add("request-content-removed", f"{where} {media}",
                             "this request body type is no longer accepted")
                continue
            self.compare(body.get("schema", {}), new_content[media].get("schema", {}),
                         {"request"}, f"{where} body {media}")

        new_responses = new.get("responses") or {}
        for status, response in sorted((old.get("responses") or {}).items()):
            if not str(status).startswith("2"):
                continue
            if status not in new_responses:
                self.add("response-removed", f"{where} {status}", "success status removed")
                continue
            new_media = new_responses[status].get("content") or {}
            for media, body in sorted((response.get("content") or {}).items()):
                if media not in new_media:
                    self.add("response-content-removed", f"{where} {status} {media}",
                             "response body type removed")
                    continue
                self.compare(body.get("schema", {}), new_media[media].get("schema", {}),
                             {"response"}, f"{where} {status} {media}")


# --- WIT ----------------------------------------------------------------------------------------

TOKEN = re.compile(r"%?[A-Za-z_][A-Za-z0-9_-]*|->|\d+\.\d+\.\d+[-+.A-Za-z0-9]*|\d+|[{}()<>,:;=.@/*_]")


def tokenize(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"//[^\n]*", " ", text)
    return TOKEN.findall(text)


class WitParser:
    """Just enough of WIT for a structural comparison; not a validator."""

    def __init__(self, text):
        self.tokens = tokenize(text)
        self.pos = 0
        self.version = None
        self.types = {}   # "iface.name" -> (kind, [(member, type)])
        self.funcs = {}   # "iface.func" or "iface.resource.method" -> signature
        self.worlds = {}  # world -> {"import:name": signature or "", "export:name": ...}
        self.interfaces = set()

    def peek(self, offset=0):
        index = self.pos + offset
        return self.tokens[index] if index < len(self.tokens) else None

    def take(self):
        token = self.peek()
        self.pos += 1
        return token

    def until(self, stops):
        """Tokens up to (not including) a stop token at nesting depth 0."""
        depth, taken = 0, []
        while self.peek() is not None:
            token = self.peek()
            if depth == 0 and token in stops:
                break
            depth += token in "<({"
            depth -= token in ">)}"
            taken.append(self.take())
        return taken

    def skip_gates(self):
        while self.peek() == "@":  # @since(...), @unstable(...), @deprecated(...)
            self.take()
            self.take()
            if self.peek() == "(":
                self.until({")"})
                self.take()

    def parse(self):
        while self.peek() is not None:
            self.skip_gates()
            token = self.take()
            if token == "package":
                decl = "".join(self.until({";"}))
                self.version = decl.split("@", 1)[1] if "@" in decl else None
                self.take()
            elif token in ("interface", "world"):
                name = self.take().lstrip("%")
                self.take()  # {
                if token == "interface":
                    self.interfaces.add(name)
                    self.parse_interface(name)
                else:
                    self.parse_world(name)
            elif token is not None:
                self.until({";"})
                self.take()
        return self

    @staticmethod
    def signature(tokens):
        """`func(a: u32, b: string) -> r` without the parameter names, which do not bind."""
        text = " ".join(tokens)
        text = re.sub(r"(\(|,)\s*%?[A-Za-z_][A-Za-z0-9_-]*\s*:", r"\1", text)
        return re.sub(r"\s+", "", text)

    def members(self):
        """`{ a: t, b(t), c, }` -> [(a, t), (b, t), (c, "")]."""
        found = []
        while self.peek() not in ("}", None):
            self.skip_gates()
            name = self.take().lstrip("%")
            if self.peek() == ":":
                self.take()
                found.append((name, "".join(self.until({",", "}"}))))
            elif self.peek() == "(":
                found.append((name, "".join(self.until({",", "}"}))))
            else:
                found.append((name, ""))
            if self.peek() == ",":
                self.take()
        self.take()
        return found

    def parse_interface(self, iface):
        while self.peek() not in ("}", None):
            self.skip_gates()
            token = self.take()
            if token in ("record", "variant", "enum", "flags"):
                name = self.take().lstrip("%")
                self.take()  # {
                self.types[f"{iface}.{name}"] = (token, self.members())
            elif token == "resource":
                name = self.take().lstrip("%")
                self.types[f"{iface}.{name}"] = ("resource", [])
                if self.take() == "{":
                    while self.peek() not in ("}", None):
                        self.skip_gates()
                        method = self.take().lstrip("%")
                        if method == "constructor":
                            sig = "constructor" + self.signature(self.until({";"}))
                        else:
                            self.take()  # :
                            sig = self.signature(self.until({";"}))
                        self.take()
                        self.funcs[f"{iface}.{name}.{method}"] = sig
                    self.take()
            elif token == "type":
                name = self.take().lstrip("%")
                self.take()  # =
                self.types[f"{iface}.{name}"] = ("type", [("=", "".join(self.until({";"})))])
                self.take()
            elif token == "use":
                self.until({";"})
                self.take()
            else:
                name = token.lstrip("%")
                self.take()  # :
                self.funcs[f"{iface}.{name}"] = self.signature(self.until({";"}))
                self.take()
        self.take()

    def parse_world(self, world):
        items = self.worlds.setdefault(world, {})
        while self.peek() not in ("}", None):
            self.skip_gates()
            token = self.take()
            if token in ("import", "export"):
                path = self.until({";"})
                # `import name: func(...)` names its item; `import ns:pkg/iface@1.0.0` is a path.
                if len(path) > 2 and path[1] == ":" and path[2] in (
                        "func", "async", "interface"):
                    items[f"{token}:{path[0].lstrip('%')}"] = self.signature(path[2:])
                else:
                    items[f"{token}:{''.join(path).lstrip('%')}"] = ""
            else:
                self.until({";"})
            self.take()
        self.take()


def version_tuple(text):
    match = re.match(r"(\d+)\.(\d+)\.(\d+)", text or "")
    return tuple(int(part) for part in match.groups()) if match else None


def wit_bump_covers(old, new):
    """A major bump, or before 1.0 a minor one, is the explicit versioning a break needs."""
    old_v, new_v = version_tuple(old), version_tuple(new)
    if not old_v or not new_v:
        return False
    if old_v[0] == 0:
        return new_v[0] > 0 or new_v[1] > old_v[1]
    return new_v[0] > old_v[0]


def compare_wit(old, new):
    findings = []

    def add(kind, location, message):
        findings.append((f"wit:{kind}:{location}", message))

    for iface in sorted(old.interfaces - new.interfaces):
        add("interface-removed", iface, "interface removed")
    exported = {item.split(":", 1)[1] for items in old.worlds.values() for item in items
                if item.startswith("export:")}

    for name, (kind, members) in sorted(old.types.items()):
        if name not in new.types:
            if name.split(".")[0] in new.interfaces:
                add("type-removed", name, f"{kind} removed")
            continue
        new_kind, new_members = new.types[name]
        if new_kind != kind:
            add("type-changed", name, f"{kind} became {new_kind}")
            continue
        if kind == "type":
            if members != new_members:
                add("type-changed", name, f"{members[0][1]} -> {new_members[0][1]}")
            continue
        noun = {"record": "field", "flags": "flag"}.get(kind, "case")
        old_map, new_map = dict(members), dict(new_members)
        for member, member_type in members:
            if member not in new_map:
                add(f"{noun}-removed", f"{name}.{member}", f"{noun} removed")
            elif new_map[member] != member_type:
                add(f"{noun}-changed", f"{name}.{member}", f"{member_type} -> {new_map[member]}")
        for member, _ in new_members:
            if member not in old_map:
                add(f"{noun}-added", f"{name}.{member}",
                    f"a component built before the new {noun} fails to instantiate")
        kept_old = [m for m, _ in members if m in new_map]
        kept_new = [m for m, _ in new_members if m in old_map]
        if kept_old != kept_new:
            add(f"{noun}s-reordered", name, "the canonical ABI reads members in order")

    for name, sig in sorted(old.funcs.items()):
        iface = name.split(".")[0]
        if name not in new.funcs:
            if iface in new.interfaces:
                add("func-removed", name, "function removed")
        elif new.funcs[name] != sig:
            add("func-changed", name, f"{sig} -> {new.funcs[name]}")
    for name in sorted(set(new.funcs) - set(old.funcs)):
        iface = name.split(".")[0]
        if iface in exported and iface in old.interfaces and name.count(".") == 1:
            add("func-added-to-export", name,
                "a plugin built before it does not export it; the host cannot bind it")

    for world, items in sorted(old.worlds.items()):
        if world not in new.worlds:
            add("world-removed", world, "world removed")
            continue
        new_items = new.worlds[world]
        for item, detail in sorted(items.items()):
            direction, target = item.split(":", 1)
            if item not in new_items:
                add(f"world-{direction}-removed", f"{world}.{target}", f"{direction} removed")
            elif new_items[item] != detail:
                add(f"world-{direction}-changed", f"{world}.{target}",
                    f"{detail} -> {new_items[item]}")
        for item in sorted(set(new_items) - set(items)):
            direction, target = item.split(":", 1)
            if direction == "export":
                add("world-export-added", f"{world}.{target}",
                    "a plugin built before it does not export it")
    return findings


# --- acknowledgements and the verdict ------------------------------------------------------------


def load_acks(path, base_version):
    """{finding: release} for every acknowledgement of a release after the base."""
    if not path:
        return {}
    try:
        with open(path, "rb") as handle:
            data = tomllib.load(handle)
    except FileNotFoundError:
        return {}
    except tomllib.TOMLDecodeError as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)
    acks = {}
    base = version_tuple(base_version) or (0, 0, 0)
    for index, entry in enumerate(data.get("break", [])):
        release, finding, reason = (entry.get(k) for k in ("release", "finding", "reason"))
        if not (isinstance(release, str) and version_tuple(release)
                and isinstance(finding, str) and finding
                and isinstance(reason, str) and reason.strip()):
            print(f"compat-check: {path}: [[break]] #{index + 1} needs release (X.Y.Z), "
                  "finding and a reason", file=sys.stderr)
            sys.exit(2)
        if version_tuple(release) > base:
            acks[finding] = release
    return acks


def read_json(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)


def read_text(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--base-version", required=True)
    parser.add_argument("--acks")
    for name in ("old-openapi", "new-openapi", "old-wit", "new-wit"):
        parser.add_argument(f"--{name}")
    args = parser.parse_args()

    findings, covered = [], []
    if args.old_openapi and args.new_openapi:
        findings += Rest(read_json(args.old_openapi), read_json(args.new_openapi)).run()
    if args.old_wit and args.new_wit:
        old = WitParser(read_text(args.old_wit)).parse()
        new = WitParser(read_text(args.new_wit)).parse()
        wit = compare_wit(old, new)
        if wit and wit_bump_covers(old.version, new.version):
            covered = wit
            print(f"wit: rdownloader:plugin@{old.version} -> @{new.version} versions "
                  f"{len(wit)} break(s)")
        else:
            findings += wit
            if old.version != new.version:
                print(f"wit: @{old.version} -> @{new.version} is not a major bump "
                      "(before 1.0: a minor one); it versions nothing")

    acks = load_acks(args.acks, args.base_version)
    unacknowledged = 0
    for key, message in covered:
        print(f"versioned  {key}  ({message})")
    for key, message in findings:
        if key in acks:
            print(f"accepted   {key}  ({message}; acknowledged for {acks[key]})")
        else:
            print(f"BREAK      {key}  ({message})")
            unacknowledged += 1
    found = {key for key, _ in findings}
    for key, release in sorted(acks.items()):
        if key not in found:
            print(f"note: the acknowledgement for {release} matches no finding: {key}")
    print(f"compat-check: {len(findings) + len(covered)} break(s) against {args.base_version}, "
          f"{unacknowledged} unacknowledged")
    return 1 if unacknowledged else 0


if __name__ == "__main__":
    sys.exit(main())
