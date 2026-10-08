# shellcheck shell=bash
#
# The workspace's crate graph, read from the manifests without cargo (RD-1150-06): the members,
# their package names, what a changed path demands of them, and the whole reverse hull of a change
# — every member that builds on it through any chain of path dependencies, not scope.sh's one
# level. For check.sh --full's reuse per crate (lib/check-reuse-crates.sh). Like scope.sh, every
# function reads the checkout in the working directory and writes its answer to stdout.
#
#   rd_crate_load_graph            # fills RD_CRATE_NAME, RD_CRATE_DIR, RD_CRATE_DEPENDANTS
#   rd_crate_reverse_hull <pkg>... # those and every member that builds on one of them, sorted
#   rd_crate_demands < paths       # what each changed path demands, one line per demand

# The member directories of the workspace's root Cargo.toml, one per line, globs expanded; each
# with a manifest of its own.
rd_crate_members() {
    local entry dir
    awk '/^members[[:space:]]*=/ { on = 1 } on { print } on && /\]/ { exit }' Cargo.toml \
        | grep -oE '"[^"]+"' | tr -d '"' | while read -r entry; do
            # shellcheck disable=SC2086 # the entry is a glob by design
            for dir in $entry; do
                [[ ! -f "$dir/Cargo.toml" ]] || printf '%s\n' "$dir"
            done
        done
}

# Fills three tables from the members' manifests: RD_CRATE_NAME (directory -> package name),
# RD_CRATE_DIR (package name -> directory) and RD_CRATE_DEPENDANTS (directory -> the directories of
# the members that name it as a path dependency, space separated). Every dependency section
# counts — normal, build, dev, target-specific — since a dependant's tests build on a dev-dependency
# too; following one on can only widen. RD_CRATE_WIDE names what keeps the graph from being whole:
# a path dependency the root manifest declares, which a member's `workspace = true` would hide.
rd_crate_load_graph() {
    local kind first second
    local -a manifests=()
    declare -gA RD_CRATE_NAME=() RD_CRATE_DIR=() RD_CRATE_DEPENDANTS=()
    RD_CRATE_WIDE=""
    mapfile -t manifests < <(rd_crate_members | sed 's|$|/Cargo.toml|')
    if awk '/^\[/ { deps = ($0 ~ /dependencies/) } deps && /path[[:space:]]*=/ { found = 1 } END { exit !found }' Cargo.toml; then
        RD_CRATE_WIDE="the root Cargo.toml declares a path dependency, which the graph does not follow"
    fi
    [[ ${#manifests[@]} -gt 0 ]] || return 0
    while read -r kind first second; do
        case "$kind" in
            package) RD_CRATE_NAME[$first]="$second"; RD_CRATE_DIR[$second]="$first" ;;
            edge) RD_CRATE_DEPENDANTS[$second]+="$first " ;;
        esac
    done < <(awk '
        function normal(path,   count, parts, kept, i, depth, joined) {
            count = split(path, parts, "/"); depth = 0
            for (i = 1; i <= count; i++) {
                if (parts[i] == "" || parts[i] == ".") continue
                if (parts[i] == "..") { if (depth > 0) depth--; continue }
                kept[++depth] = parts[i]
            }
            joined = ""
            for (i = 1; i <= depth; i++) joined = joined (i > 1 ? "/" : "") kept[i]
            return joined
        }
        FNR == 1 { dir = FILENAME; sub(/\/Cargo\.toml$/, "", dir); section = "" }
        /^\[/ { section = $0 }
        section == "[package]" && /^name[[:space:]]*=/ {
            name = $0; sub(/^[^"]*"/, "", name); sub(/".*/, "", name); print "package", dir, name
        }
        section ~ /dependencies/ && match($0, /path[[:space:]]*=[[:space:]]*"[^"]*"/) {
            path = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", path); sub(/"$/, "", path)
            print "edge", dir, normal(dir "/" path)
        }' "${manifests[@]}")
}

# The packages named as arguments and every member that builds on one of them, one per line,
# sorted bytewise. Needs rd_crate_load_graph.
rd_crate_reverse_hull() {
    local name dir dependant
    local -A seen=()
    local -a queue=("$@")
    while [[ ${#queue[@]} -gt 0 ]]; do
        name="${queue[0]}"
        queue=("${queue[@]:1}")
        [[ -n "$name" && -z "${seen[$name]:-}" ]] || continue
        seen[$name]=1
        dir="${RD_CRATE_DIR[$name]:-}"
        [[ -n "$dir" ]] || continue
        for dependant in ${RD_CRATE_DEPENDANTS[$dir]:-}; do
            queue+=("${RD_CRATE_NAME[$dependant]}")
        done
    done
    [[ ${#seen[@]} -eq 0 ]] || printf '%s\n' "${!seen[@]}" | LC_ALL=C sort
}

# The member directory path $1 lies in (the longest), or nothing. Needs rd_crate_load_graph.
rd_crate_dir_of() {
    local path="$1" prefix="" part found=""
    local -a parts
    IFS=/ read -r -a parts <<< "$path"
    for part in "${parts[@]}"; do
        prefix="${prefix:+$prefix/}$part"
        [[ -z "${RD_CRATE_NAME[$prefix]:-}" ]] || found="$prefix"
    done
    printf '%s\n' "$found"
}

# What a changed path cannot be placed in, as a reason; nothing when it can. These govern what
# every member builds (a manifest, the lock file, the toolchain, cargo's, nextest's and
# cargo-deny's configuration, a build script), or reach every member anyway (rd-core and the
# plugin types it re-exports, the migrations) — the whole Rust half runs, as the job says.
rd_crate_wide_reason() {
    case "$1" in
        Cargo.toml|Cargo.lock|*/Cargo.toml|*/Cargo.lock) echo "$1: a manifest or the lock file decides what every member builds" ;;
        rust-toolchain.toml|.cargo/*|.config/nextest.toml|deny.toml) echo "$1 governs the whole build" ;;
        build.rs|*/build.rs) echo "$1: a build script" ;;
        crates/rd-core/*) echo "$1: rd-core, which every member builds on" ;;
        crates/rd-plugin-types/*) echo "$1: rd-plugin-types, which rd-core re-exports to every member" ;;
        crates/rd-db/migrations/*|*.sql) echo "$1: a migration" ;;
    esac
}

# Whether path $2, under src/ of the member in directory $1, is compiled only for that member's
# own tests: its module — the file's own when it is named `tests` or `*_tests`, else the nearest
# directory above it named so — is declared at least once in $1/src, and every declaration that
# may name it carries `#[cfg(test)]` or `#[cfg(all(…, test, …))]` (never `not(`). An inline
# `mod tests { … }` declares no file, so a file only it would reach does not qualify.
rd_crate_test_module() {
    local dir="$1" path="$2" module="" index declarations
    local -a parts
    [[ "$path" == "$dir/src/"*.rs ]] || return 1
    IFS=/ read -r -a parts <<< "${path#"$dir/src/"}"
    parts[-1]="${parts[-1]%.rs}"
    [[ "${parts[-1]}" != mod ]] || unset 'parts[-1]'
    for (( index = ${#parts[@]} - 1; index >= 0; index-- )); do
        if [[ "${parts[index]}" =~ ^(tests|[a-z0-9_]+_tests)$ ]]; then module="${parts[index]}"; break; fi
    done
    [[ -n "$module" ]] || return 1
    declarations="$(find "$dir/src" -name '*.rs' -exec awk -v module="$module" '
        FNR == 1 { attributes = "" }
        /^[[:space:]]*#\[/ { attributes = attributes " " $0; next }
        /^[[:space:]]*((pub(\([^)]*\))?)[[:space:]]+)?mod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*;/ {
            name = $0
            sub(/^[[:space:]]*((pub(\([^)]*\))?)[[:space:]]+)?mod[[:space:]]+/, "", name)
            sub(/[[:space:]]*;.*/, "", name)
            path = ""
            if (match(attributes, /#\[path[[:space:]]*=[[:space:]]*"[^"]*"/)) {
                path = substr(attributes, RSTART, RLENGTH); sub(/^[^"]*"/, "", path); sub(/"$/, "", path)
            }
            if ((path == "" && name == module) \
                || (path != "" && path ~ ("(^|/)" module "(\\.rs|/mod\\.rs)$"))) {
                gated = attributes ~ /#\[cfg\((test\)|all\(([^]]*[ ,(])?test[ ,)])/ && attributes !~ /not\(/
                print (gated ? "gated" : "open")
            }
            attributes = ""
            next
        }
        /^[[:space:]]*(\/\/.*)?$/ { next }
        { attributes = "" }' {} +)"
    [[ -n "$declarations" && "$declarations" != *open* ]]
}

# Whether path $2 of the member in directory $1 reaches that member's own tests only: a file under
# its tests/, benches/ or examples/ that no other source of it names but a test module (an
# include_str! of a fixture from library code makes it the library's), or a test module file
# (rd_crate_test_module).
rd_crate_test_only() {
    local dir="$1" path="$2" relative file
    relative="${path#"$dir/"}"
    case "$relative" in
        tests/*|benches/*|examples/*)
            while IFS=: read -r file _; do
                [[ "$file" =~ ^$dir/(tests|benches|examples)/ ]] || rd_crate_test_module "$dir" "$file" || return 1
            done < <(rd_crate_code_mentions "$dir" "$relative" "$(rd_crate_named_dir "$relative")")
            ;;
        *) rd_crate_test_module "$dir" "$path" ;;
    esac
}

# The line ranges `<mod line> <closing line>` of the top-level `#[cfg(test)] mod … {` blocks of
# the Rust text on stdin, as rustfmt lays them out — the attribute and the `mod` line at column 0,
# the closing brace alone at column 0; `cargo fmt --check` runs in the same --full. A `}` at
# column 0 inside a raw string ends a block early, which only makes less of the file test code.
rd_rust_test_blocks() {
    awk '
        /^#\[cfg\((test\)|all\(([^]]*[ ,(])?test[ ,)])/ && !/not\(/ { pending = 1; next }
        pending && /^#\[/ { next }
        pending && /^(pub(\([^)]*\))? )?mod [A-Za-z0-9_]+ \{$/ { start = NR; pending = 0; next }
        { pending = 0 }
        start && /^\}$/ { print start, NR; start = 0 }'
}

# Whether hunk side $1 (`<start>[,<count>]` of a -U0 hunk header) lies inside the body of one of
# the blocks $2 (rd_rust_test_blocks): changed lines strictly between the `mod` line and the
# closing brace, an insertion (count 0, after line <start>) no further out than right after the one
# and right before the other.
rd_hunk_in_blocks() {
    local start="${1%%,*}" count=1 open close
    [[ "$1" != *,* ]] || count="${1#*,}"
    while read -r open close; do
        [[ -n "$open" ]] || continue
        if [[ "$count" -eq 0 ]]; then
            (( start >= open && start + 1 <= close )) && return 0
        else
            (( start > open && start + count - 1 < close )) && return 0
        fi
    done <<< "$2"
    return 1
}

# Whether every hunk of the change to Rust file $3 between trees (or commits) $1 and $2 lies inside
# the body of a top-level #[cfg(test)] module on both sides: an edited unit test, which only its
# own crate's test build compiles. A file missing on either side does not qualify.
rd_crate_test_hunks() {
    local from="$1" to="$2" path="$3" old new hunks old_side new_side
    old="$(git show "$from:$path" 2> /dev/null | rd_rust_test_blocks)"
    new="$(git show "$to:$path" 2> /dev/null | rd_rust_test_blocks)"
    [[ -n "$old" && -n "$new" ]] || return 1
    hunks="$(git diff -U0 "$from" "$to" -- "$path" | sed -n 's/^@@ -\([0-9,]*\) +\([0-9,]*\) @@.*/\1 \2/p')"
    [[ -n "$hunks" ]] || return 1
    while read -r old_side new_side; do
        rd_hunk_in_blocks "$old_side" "$old" && rd_hunk_in_blocks "$new_side" "$new" || return 1
    done <<< "$hunks"
}

# The directory of member-relative path $1 with its trailing slash when it is at least two deep
# (`tests/fixtures/`), so a reader that joins the file name on at run time is still seen; the path
# itself otherwise — `tests/` or `src/` alone would match every comment that names the layout.
rd_crate_named_dir() {
    local directory="${1%/*}"
    if [[ "$directory" != "$1" && "$directory" == */* ]]; then printf '%s/\n' "$directory"; else printf '%s\n' "$1"; fi
}

# What each changed path on stdin (relative to the checkout, Rust inputs only) demands — between
# trees RD_CRATE_FROM and RD_CRATE_TO when set, which lets an edit inside a #[cfg(test)] module
# count as test code (rd_crate_test_hunks) — one line per demand: `wide <reason>` — the whole Rust half; `hull <package> <path>` — that package and
# everything that builds on it; `own <package> <path>` — that package's own tests. Beyond the
# member a path lies in:
#   * a non-test path under plugins/ or the WIT contract also demands the contract tests of
#     rd-plugin-ext and rd-plugin-host, as at branch level (they run the built components);
#   * another member whose sources name the path, or its directory two deep, outside a comment
#     reads it (`include_str!("../../turbobit/tests/fixtures/…")`): its own tests when the naming
#     file is test code of it, its hull otherwise;
#   * a path outside every member is demanded for the packages scripts/lib/rust-test-inputs.map
#     names, and is `wide` when it names none.
# Needs rd_crate_load_graph and scope.sh's rd_scope_input_packages.
rd_crate_demands() {
    local path dir name relative package file needle reason
    local -a patterns=() members=()
    [[ -z "$RD_CRATE_WIDE" ]] || echo "wide $RD_CRATE_WIDE"
    while read -r path; do
        [[ -n "$path" ]] || continue
        reason="$(rd_crate_wide_reason "$path")"
        if [[ -n "$reason" ]]; then echo "wide $reason"; continue; fi
        dir="$(rd_crate_dir_of "$path")"
        if [[ -z "$dir" ]]; then
            mapfile -t members < <(rd_scope_input_packages <<< "$path")
            [[ ${#members[@]} -gt 0 ]] || { echo "wide $path: read by the Rust half, but no crate is known to read it"; continue; }
            for package in "${members[@]}"; do echo "hull $package $path"; done
            continue
        fi
        name="${RD_CRATE_NAME[$dir]}"
        relative="${path#"$dir/"}"
        patterns+=("${dir##*/}/$relative" "${dir##*/}/$(rd_crate_named_dir "$relative")")
        if rd_crate_test_only "$dir" "$path" || { [[ -n "${RD_CRATE_FROM:-}" && "$path" == *.rs ]] \
            && rd_crate_test_hunks "$RD_CRATE_FROM" "$RD_CRATE_TO" "$path"; }; then
            echo "own $name $path"
            continue
        fi
        echo "hull $name $path"
        if [[ "$path" == plugins/* || "$path" == crates/rd-plugin-api/wit/* ]]; then
            for package in rd-plugin-ext rd-plugin-host; do
                [[ -z "${RD_CRATE_DIR[$package]:-}" ]] || echo "hull $package $path"
            done
        fi
    done
    [[ ${#patterns[@]} -gt 0 ]] || return 0
    while IFS=: read -r file needle; do
        dir="$(rd_crate_dir_of "$file")"
        [[ -n "$dir" && "${dir##*/}" != "${needle%%/*}" ]] || continue
        if [[ "$file" =~ ^$dir/(tests|benches|examples)/ ]] || rd_crate_test_module "$dir" "$file"; then
            echo "own ${RD_CRATE_NAME[$dir]} $file names $needle"
        else
            echo "hull ${RD_CRATE_NAME[$dir]} $file names $needle"
        fi
    done < <(mapfile -t members < <(printf '%s\n' "${!RD_CRATE_NAME[@]}" | LC_ALL=C sort)
        rd_crate_code_mentions "${members[@]}" -- "${patterns[@]}" | LC_ALL=C sort -u)
}

# Where the Rust sources under the directories before `--` name one of the strings after it
# outside a comment line, as `<file>:<string>` lines; with a single directory, `--` may be left
# out (`rd_crate_code_mentions <dir> <string>...`). A string in a `//` line is prose, not a read.
rd_crate_code_mentions() {
    local -a directories=() strings=()
    if [[ " $* " == *" -- "* ]]; then
        while [[ "$1" != -- ]]; do directories+=("$1"); shift; done
        shift
    else
        directories=("$1")
        shift
    fi
    strings=("$@")
    [[ ${#directories[@]} -gt 0 && ${#strings[@]} -gt 0 ]] || return 0
    grep -rHF --include='*.rs' -f <(printf '%s\n' "${strings[@]}") "${directories[@]}" 2> /dev/null \
        | RD_MENTION_STRINGS="$(printf '%s\n' "${strings[@]}")" awk '
            BEGIN { count = split(ENVIRON["RD_MENTION_STRINGS"], wanted, "\n") }
            {
                file = $0; sub(/:.*/, "", file)
                line = substr($0, length(file) + 2)
                if (line ~ /^[[:space:]]*\/\//) next
                for (i = 1; i <= count; i++) if (wanted[i] != "" && index(line, wanted[i])) print file ":" wanted[i]
            }' || true
}
