"""The plugin-contract half of scripts/lib/compat-check.py (RD-170-08): just enough of a WIT
parser for a structural comparison, the breaks between two versions of
`crates/rd-plugin-api/wit/rdownloader.wit`, and the version bump that versions them."""

import re

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
