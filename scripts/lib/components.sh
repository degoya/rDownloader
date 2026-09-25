#!/usr/bin/env bash
# shellcheck shell=bash
#
# The three plugin component gates of scripts/check.sh, kept here so check.sh stays readable.
# They run before anything expensive: each is a stat, a hash or an archive read, and each stops
# a run that would otherwise fail forty minutes later on a component that is not what the
# sources say.
#
# Expects from the caller: `step`, `touches`, `full`, `changed` and `TARGET_DIR`, as check.sh
# defines them, `rd_plugin_linked_crates` from lib/scope.sh, and the working directory at the
# checkout root.

rd_component_gates() {
    local absent stale unbumped unbumped_scope linked_touched directory name version member package
    local unbumped_names=()
    # Before anything expensive: target/ is per checkout and `cargo test` never builds
    # components, so a merge leaves the previous component next to the current sources and the
    # plugin contract tests fail on behaviour that was fixed long ago. They now say so
    # themselves, but they say it forty minutes into this script; here it costs a stat.
    #
    # Two states, not one. A component that is *missing* used to be the quiet one: the loader
    # returned early, every contract test passed, and the number at the end of the run said
    # nothing about the fact that no component had been loaded at all. Since RD-108-16 such a
    # test fails — and this check is why it fails here, in a second, rather than after the
    # build.
    step "plugin components exist"
    absent="$(scripts/build-plugins.sh --list-missing)"
    if [[ -n "$absent" ]]; then
        echo "!! these components have never been built in this checkout:" >&2
        echo "     $(tr '\n' ' ' <<< "$absent")" >&2
        echo >&2
        echo "   Build and stamp them (self-locking; never wrap it in flock):" >&2
        echo "     scripts/build-plugins.sh --components-only" >&2
        echo "   The contract tests need them and fail without them." >&2
        echo "   Without a wasm toolchain here, leave those tests out on purpose:" >&2
        echo "     cargo nextest run -P no-components --workspace" >&2
        exit 1
    fi
    echo "    every bundled component is built"

    # By content since RD-120-58: a stamp beside each component records the hash of the
    # sources it was built from and of the component itself. File times said "stale" after
    # every checkout; this says it only when the content differs, or when a component was
    # rebuilt by a bare `cargo component build` that wrote no stamp.
    step "plugin components against their sources"
    stale="$(scripts/build-plugins.sh --list-stale)"
    if [[ -n "$stale" ]]; then
        echo "!! these components were not built from the sources in this checkout:" >&2
        echo "     $(tr '\n' ' ' <<< "$stale")" >&2
        echo >&2
        echo "   Rebuild and stamp every stale or missing one (self-locking; never wrap it in flock):" >&2
        echo "     scripts/build-plugins.sh --components-only" >&2
        echo "   The contract tests would fail on them later in this run." >&2
        exit 1
    fi
    echo "    every built component carries the stamp of these sources"

    # Same version, same content (RD-120-47): a plugin that changed and kept its version is
    # never taken by an installation, which only takes a newer bundled one. build-plugins.sh
    # refuses to sign such a package; this asks the same question without signing, against the
    # built components and the signed set in the main checkout's dist/plugins. It needs the
    # components current, which is why it comes after the staleness check.
    #
    # Scoped to the plugins the change touches: target/ is shared between worktrees, so the
    # component of a plugin this branch never touched may be another branch's work and differ
    # for that reason alone. A shared plugin library, the WIT contract or a workspace crate the
    # plugins link (rd_plugin_linked_crates: rd-core, rd-plugin-api and what they pull in) can
    # change any component, so each of them asks about all of them, and so does --full. The
    # linked crates were missing until 1.2.3: rd-plugin-api changed in 1.2.2, 72 components
    # with it, and the branch check said "no plugin in the change set".
    step "plugin content against the signed package of the same version"
    unbumped_scope="the plugins this change touches"
    linked_touched=""
    while read -r directory; do
        [[ -n "$directory" ]] || continue
        if grep -q "^$directory" <<< "$changed"; then
            linked_touched="$directory"
            break
        fi
    done < <(rd_plugin_linked_crates)
    if [[ "$full" -eq 1 ]] || touches '^crates/rd-plugin-api/wit/'; then
        unbumped_scope="every plugin"
    elif [[ -n "$linked_touched" ]]; then
        unbumped_scope="every plugin, since the change touches ${linked_touched%/}, which plugins link"
    else
        while read -r directory; do
            [[ -n "$directory" ]] || continue
            if [[ ! -f "plugins/$directory/manifest.toml" ]]; then
                unbumped_scope="every plugin"
                unbumped_names=()
                break
            fi
            unbumped_names+=("$directory")
        done < <(sed -n 's|^plugins/\([^/]*\)/.*|\1|p' <<< "$changed" | sort -u)
    fi
    if [[ "$unbumped_scope" != "every plugin"* && ${#unbumped_names[@]} -eq 0 ]]; then
        echo "    no plugin in the change set"
    else
        unbumped="$(scripts/build-plugins.sh --list-unbumped "${unbumped_names[@]+"${unbumped_names[@]}"}")"
        if [[ -n "$unbumped" ]]; then
            echo "!! these plugins changed but kept a version that is already signed:" >&2
            echo >&2
            while read -r name version member package; do
                echo "     $name $version — $member differs from $package" >&2
            done <<< "$unbumped"
            echo >&2
            echo "   An installation only takes a bundled package whose version is newer than the" >&2
            echo "   one it has, so it would keep running the old code. Raise \`version\` in" >&2
            echo "   plugins/<name>/manifest.toml, in the same commit as the change." >&2
            echo "   If this checkout never changed that plugin, its component in $TARGET_DIR" >&2
            echo "   was built by another checkout: rebuild it from here and run again," >&2
            echo "     scripts/build-plugins.sh --components-only <name>" >&2
            exit 1
        fi
        echo "    checked $unbumped_scope: nothing changed under a signed version"
    fi
}
