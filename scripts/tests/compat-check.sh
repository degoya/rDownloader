#!/usr/bin/env bash
#
# The breaking-change gate (RD-170-08): scripts/lib/compat-check.py against the fixture contract
# in scripts/tests/fixtures/compat/ — one small OpenAPI document and one WIT file — changed one
# rule at a time. Each break has to be found under its exact finding string and exit 1; an
# addition has to pass; an acknowledged break and a WIT break under a large enough version bump
# have to pass. Then scripts/compat-check.sh in a scratch repository: the base it picks, the
# acknowledgement file it reads, and the refusal without a tag.
#
# Pure python3, git and bash: it runs in a second or two.
#
#   scripts/tests/compat-check.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURES="$ROOT/scripts/tests/fixtures/compat"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
export PYTHONDONTWRITEBYTECODE=1

# openapi <python statements on the parsed document `d`>: the fixture, changed, as new.json.
openapi() {
    python3 - "$FIXTURES/base.json" "$SCRATCH/new.json" "$1" <<'EOF'
import json, sys
d = json.load(open(sys.argv[1]))
schemas = d["components"]["schemas"]
exec(sys.argv[3])
json.dump(d, open(sys.argv[2], "w"))
EOF
}
# wit <sed script>: the fixture, changed, as new.wit.
wit() { sed "$1" "$FIXTURES/base.wit" > "$SCRATCH/new.wit"; }

rest() {
    run_status python3 "$ROOT/scripts/lib/compat-check.py" --base-version 1.0.0 \
        --old-openapi "$FIXTURES/base.json" --new-openapi "$SCRATCH/new.json" "$@"
}
plugin() {
    run_status python3 "$ROOT/scripts/lib/compat-check.py" --base-version 1.0.0 \
        --old-wit "$FIXTURES/base.wit" --new-wit "$SCRATCH/new.wit"
}
# breaks <name> <finding>: the last run found this break and refused.
breaks() {
    expect_status "$1: refused" 1
    expect_output "$1: named" "BREAK      $2"
}

# --- REST -------------------------------------------------------------------------------------

openapi 'pass'
rest
expect_status "REST: an unchanged document passes" 0
expect_output "and says so" "0 break(s) against 1.0.0, 0 unacknowledged"

openapi 'del d["paths"]["/api/v1/items/{id}"]'
rest
breaks "REST: a removed path" "rest:path-removed:/api/v1/items/{id}"

openapi 'del d["paths"]["/api/v1/items/{id}"]["delete"]'
rest
breaks "REST: a removed method" "rest:operation-removed:DELETE /api/v1/items/{id}"

openapi 'del schemas["Item"]["properties"]["name"]; schemas["Item"]["required"].remove("name")'
rest
breaks "REST: a removed response property" "rest:response-property-removed:schemas.Item.name"

openapi 'schemas["Item"]["required"].remove("name")'
rest
breaks "REST: a response property made optional" "rest:response-property-optional:schemas.Item.name"

openapi 'schemas["CreateItem"]["required"].append("note")'
rest
breaks "REST: a request property made required" "rest:request-property-required:schemas.CreateItem.note"

openapi 'schemas["CreateItem"]["properties"]["tag"] = {"type": "string"}; schemas["CreateItem"]["required"].append("tag")'
rest
breaks "REST: a new required request property" "rest:request-property-required:schemas.CreateItem.tag"

openapi 'd["paths"]["/api/v1/items"]["get"]["parameters"][0]["required"] = True'
rest
breaks "REST: a parameter made required" "rest:parameter-required:GET /api/v1/items query.limit"

openapi 'schemas["ItemState"]["enum"].remove("done")'
rest
breaks "REST: a narrowed enum" "rest:enum-narrowed:schemas.ItemState"

openapi 'schemas["Item"]["properties"]["id"] = {"type": "integer"}'
rest
breaks "REST: a changed type" "rest:type-changed:schemas.Item.id"

openapi 'schemas["Item"]["properties"]["name"] = {"type": ["string", "null"]}'
rest
breaks "REST: a response value that may now be null" "rest:response-nullable:schemas.Item.name"

openapi 'd["paths"]["/api/v1/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] = {"type": "array", "items": {"$ref": "#/components/schemas/Item"}}'
rest
breaks "REST: a response of another shape" "rest:type-changed:GET /api/v1/items 200 application/json"

openapi '
d["paths"]["/api/v1/tags"] = {"get": {"responses": {"200": {"description": "OK"}}}}
schemas["Item"]["properties"]["added"] = {"type": "string"}
schemas["Item"]["required"].append("added")
schemas["CreateItem"]["properties"]["colour"] = {"type": ["string", "null"]}
schemas["ItemState"]["enum"].append("failed")
d["paths"]["/api/v1/items"]["get"]["parameters"].append({"name": "q", "in": "query", "required": False, "schema": {"type": "string"}})
schemas["CreateItem"]["required"] = []'
rest
expect_status "REST: additions and a loosened request pass" 0

openapi '
item = d["paths"].pop("/api/v1/items/{id}")
for op in item.values():
    op["parameters"][0]["name"] = "item_id"
d["paths"]["/api/v1/items/{item_id}"] = item
schemas["Entry"] = schemas.pop("Item")
text = json.dumps(d).replace("schemas/Item\"", "schemas/Entry\"")
d.clear(); d.update(json.loads(text))'
rest
expect_status "REST: a renamed path parameter and a renamed schema of the same shape pass" 0

openapi 'schemas["Item"]["properties"]["size"] = {"oneOf": [{"type": "null"}, {"$ref": "#/components/schemas/ItemState"}]}'
rest
breaks "REST: a type change behind a nullable reference" "rest:type-changed:schemas.Item.size"

# --- acknowledgements --------------------------------------------------------------------------

openapi 'del d["paths"]["/api/v1/items/{id}"]["delete"]'
cat > "$SCRATCH/acks.toml" <<'EOF'
[[break]]
release = "1.1.0"
finding = "rest:operation-removed:DELETE /api/v1/items/{id}"
reason = "Items are archived, never deleted, since 1.1.0."
EOF
rest --acks "$SCRATCH/acks.toml"
expect_status "an acknowledged break passes" 0
expect_output "and is shown as accepted" "accepted   rest:operation-removed:DELETE /api/v1/items/{id}"

sed -i 's/^release = .*/release = "1.0.0"/' "$SCRATCH/acks.toml"
rest --acks "$SCRATCH/acks.toml"
breaks "an acknowledgement for the base release itself counts for nothing" \
    "rest:operation-removed:DELETE /api/v1/items/{id}"

printf '[[break]]\nrelease = "1.1.0"\nfinding = "rest:path-removed:/x"\n' > "$SCRATCH/acks.toml"
rest --acks "$SCRATCH/acks.toml"
expect_status "an acknowledgement without a reason is refused" 2
expect_output "naming what it needs" "needs release (X.Y.Z), finding and a reason"

# --- WIT ----------------------------------------------------------------------------------------

wit 's/^package .*/&/'
plugin
expect_status "WIT: an unchanged contract passes" 0

wit '/  wait: func/d'
plugin
breaks "WIT: a removed function" "wit:func-removed:host.wait"

wit 's/  log: func/  write-log: func/'
plugin
breaks "WIT: a renamed function" "wit:func-removed:host.log"

wit 's/wait: func(seconds: u32)/wait: func(seconds: u64)/'
plugin
breaks "WIT: a re-typed function" "wit:func-changed:host.wait"

wit '/    code: option<string>,/d'
plugin
breaks "WIT: a removed record field" "wit:field-removed:types.failure.code"

wit 's/    code: option<string>,/    code: string,/'
plugin
breaks "WIT: a re-typed record field" "wit:field-changed:types.failure.code"

wit 's/    code: option<string>,/&\n    params: list<tuple<string, string>>,/'
plugin
breaks "WIT: an added record field (binary-breaking)" "wit:field-added:types.failure.params"

wit '/^    permanent,$/d'
plugin
breaks "WIT: a removed variant case" "wit:case-removed:types.failure-kind.permanent"

wit 's/^    offline,$/&\n    cached,/'
plugin
breaks "WIT: an added enum case (binary-breaking)" "wit:case-added:types.link-status.cached"

wit '/^  import host;$/d'
plugin
breaks "WIT: a removed world import" "wit:world-import-removed:resolver-plugin.host"

wit 's/^  export resolver;$/&\n  export host;/'
plugin
breaks "WIT: a new export of an existing world" "wit:world-export-added:resolver-plugin.host"

wit 's/^  check: func.*/&\n  hosters: func() -> list<string>;/'
plugin
breaks "WIT: a new function on an exported interface" "wit:func-added-to-export:resolver.hosters"

wit 's/wait: func(seconds: u32)/wait: func(secs: u32)/'
plugin
expect_status "WIT: a renamed parameter passes" 0

wit 's/^  wait: func.*/&\n  now: func() -> u64;/; $a\
\
interface extra {\
  ping: func();\
}\
\
world extra-plugin {\
  import host;\
  export extra;\
}'
plugin
expect_status "WIT: a new function on an imported interface, a new interface and world pass" 0

wit '/  wait: func/d; s/@0.9.0/@0.10.0/'
plugin
expect_status "WIT: a break under a minor bump before 1.0 passes" 0
expect_output "as versioned" "versioned  wit:func-removed:host.wait"

wit '/  wait: func/d; s/@0.9.0/@0.9.1/'
plugin
breaks "WIT: a patch bump versions nothing" "wit:func-removed:host.wait"

sed 's/@0.9.0/@1.2.0/' "$FIXTURES/base.wit" > "$SCRATCH/one.wit"
sed '/  wait: func/d; s/@0.9.0/@1.3.0/' "$FIXTURES/base.wit" > "$SCRATCH/new.wit"
run_status python3 "$ROOT/scripts/lib/compat-check.py" --base-version 1.0.0 \
    --old-wit "$SCRATCH/one.wit" --new-wit "$SCRATCH/new.wit"
breaks "WIT: after 1.0 a minor bump versions nothing" "wit:func-removed:host.wait"
sed '/  wait: func/d; s/@0.9.0/@2.0.0/' "$FIXTURES/base.wit" > "$SCRATCH/new.wit"
run_status python3 "$ROOT/scripts/lib/compat-check.py" --base-version 1.0.0 \
    --old-wit "$SCRATCH/one.wit" --new-wit "$SCRATCH/new.wit"
expect_status "WIT: after 1.0 a major bump versions the break" 0

# --- scripts/compat-check.sh in a repository ----------------------------------------------------

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib" "$TREE/web" "$TREE/crates/rd-plugin-api/wit"
cp "$ROOT/scripts/compat-check.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{compat-check,compat_rest,compat_wit}.py "$ROOT/scripts/lib/release-tag.sh" \
    "$ROOT/scripts/lib/workspace-version.sh" "$TREE/scripts/lib/"
cp "$FIXTURES/base.json" "$TREE/web/openapi.json"
cp "$FIXTURES/base.wit" "$TREE/crates/rd-plugin-api/wit/rdownloader.wit"
cargo_version() { printf '[workspace.package]\nversion = "%s"\n' "$1" > "$TREE/Cargo.toml"; }
git init -q -b development "$TREE"

run_status "$TREE/scripts/compat-check.sh"
expect_status "the gate without any release tag: refused, not passed" 2
expect_output "saying what is missing" "no release tag vX.Y.Z"

cargo_version 1.0.0
git -C "$TREE" add -A && git -C "$TREE" commit -qm "release 1.0.0" && git -C "$TREE" tag v1.0.0
python3 -c '
import json, sys
d = json.load(open(sys.argv[1])); del d["paths"]["/api/v1/items/{id}"]
json.dump(d, open(sys.argv[1], "w"))' "$TREE/web/openapi.json"
cargo_version 1.1.0
git -C "$TREE" commit -qam "work towards 1.1.0"
# A tag above the workspace version is a release this tree does not come after.
git -C "$TREE" tag v2.0.0

run_status "$TREE/scripts/compat-check.sh"
breaks "the default base: the highest tag not above the workspace version" \
    "rest:path-removed:/api/v1/items/{id}"
expect_output "named as the base" "compat-check: v1.0.0 (1.0.0) -> working tree"

run_status "$TREE/scripts/compat-check.sh" --base v2.0.0
expect_status "--base names another ref" 0

cat > "$TREE/scripts/compat-breaks.toml" <<'EOF'
[[break]]
release = "1.1.0"
finding = "rest:path-removed:/api/v1/items/{id}"
reason = "Items are read from the list only."
EOF
run_status "$TREE/scripts/compat-check.sh"
expect_status "the acknowledgement file under scripts/ is read" 0

finish_tests compat-check
