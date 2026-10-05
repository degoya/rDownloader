"""The REST half of scripts/lib/compat-check.py (RD-170-08): the breaks between two versions of
`web/openapi.json`, as `rest:<kind>:<location>` findings. The rules are in compat-check.py's
docstring and docs/plugins.md#compatibility."""

import json
import re

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
