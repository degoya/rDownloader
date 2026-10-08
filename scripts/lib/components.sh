#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # `changed` and `full` are check.sh's, which sources this file
#
# The plugin component gates of scripts/check.sh, kept here so check.sh stays readable. They run
# before anything expensive: a component that is missing or not what the sources say is built
# (RD-1100-13), and a plugin that changed under a signed version stops the run — either would
# otherwise fail it forty minutes later.
#
# Expects from the caller: `step`, `touches`, `full`, `changed` and `TARGET_DIR`, as check.sh
# defines them, `rd_plugin_linked_crates` from lib/scope.sh, and the working directory at the
# checkout root.

rd_component_gates() {
    local build unbumped unbumped_scope linked_touched directory name version member package
    local unbumped_names=() names=()
    # Before anything expensive: target/ is per checkout and `cargo test` never builds
    # components, so a merge leaves the previous component next to the current sources and the
    # plugin contract tests fail on behaviour that was fixed long ago. They now say so
    # themselves, but they say it forty minutes into this script; here it costs a stat.
    #
    # Two states, not one. A component that is *missing* used to be the quiet one: the loader
    # returned early, every contract test passed, and the number at the end of the run said
    # nothing about the fact that no component had been loaded at all. Since RD-108-16 such a
    # test fails. Staleness is by content since RD-120-58: a stamp beside each component records
    # the hash of the sources it was built from and of the component itself, so "stale" means
    # the content differs, or a bare `cargo build` relinked it without a stamp.
    #
    # Both are built here, under this run's lock (RD-1100-13): build-plugins.sh sees RD_LOCK_HELD
    # and takes no lock of its own. Until 1.10 the run stopped with the command to type, and the
    # integration of a wave that changed a plugin paid a second start for it.
    step "plugin components: built, and from these sources"
    build="$( (scripts/build-plugins.sh --list-missing; scripts/build-plugins.sh --list-stale) | sort -u)"
    if [[ -n "$build" ]]; then
        mapfile -t names <<< "$build"
        echo "    building ${#names[@]} missing or stale: ${names[*]}"
        if ! scripts/build-plugins.sh --components-only "${names[@]}"; then
            echo "!! these components did not build: ${names[*]}" >&2
            echo "   The contract tests need them and fail without them. Without a wasm toolchain" >&2
            echo "   here, leave those tests out on purpose:" >&2
            echo "     cargo nextest run -P no-components --workspace" >&2
            exit 1
        fi
    fi
    echo "    every bundled component is built and carries the stamp of these sources"

    # Same version, same content (RD-120-47): a plugin that changed and kept its version is
    # never taken by an installation, which only takes a newer bundled one. build-plugins.sh
    # refuses to sign such a package; this asks the same question without signing, against the
    # built components and the signed set in the main checkout's dist/plugins. It needs the
    # components current, which is why it comes after the staleness check.
    #
    # Scoped to the plugins the change touches: target/ is shared between worktrees, so the
    # component of a plugin this branch never touched may be another branch's work and differ
    # for that reason alone. A shared plugin library, the WIT contract or a workspace crate the
    # plugins link (rd_plugin_linked_crates: rd-plugin-api, rd-plugin-types and what they pull
    # in) can change any component, so each of them asks about all of them, and so does --full. The
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
