# shellcheck shell=bash
# The one layout of a release archive (RD-180-05), local and in CI alike: flat, no folder above
# the files, and nothing but the entries below plus plugins/ with the signed packages.
#
# Usage (sourced):
#   rd_archive_entries <linux|windows|macos>             # the files, one per line
#   rd_check_archive_layout <archive> <linux|windows|macos>
#
# Until 1.8 scripts/package-linux.sh and package-windows.sh packed their folder itself
# (`linux/rdownloader`, `windows/rdownloader.exe`) while release.yml packed flat archives, so a
# local package and the published one unpacked differently, and only the published one carried
# no VERSION.txt. CI's content is the reference: the helper binaries in artifacts/<platform>/vendor
# stay in the local folder and never reach an archive, as the published archive has never carried
# them (the managed tool store fetches them). UNVERIFIED.txt is the one local extra, allowed so a
# package built without a --full green keeps saying so (scripts/package-windows.sh).

rd_archive_entries() {
    case "$1" in
        linux)
            printf '%s\n' LICENSE README.md VERSION.txt rdownloader rdownloader-capture \
                start-capture.sh start-rdownloader.sh stop-capture.sh stop-rdownloader.sh
            ;;
        windows)
            printf '%s\n' LICENSE README.md VERSION.txt rdownloader.exe rdownloader-capture.exe \
                start-capture.bat start-rdownloader.bat stop-capture.bat stop-rdownloader.bat
            ;;
        macos)
            printf '%s\n' LICENSE README.md VERSION.txt rdownloader rdownloader-capture \
                start-capture.command start-rdownloader.command stop-capture.command \
                stop-rdownloader.command "rDownloader Capture.app"
            ;;
        *)
            echo "rd_archive_entries: unknown platform '$1' (linux, windows or macos)" >&2
            return 2
            ;;
    esac
}

# Lists every member of a .tar.gz or .zip, one per line, without a leading `./`. Python rather
# than tar/unzip: the same code reads both formats on every runner.
rd_archive_members() {
    python3 - "$1" <<'PY'
import sys, tarfile, zipfile
path = sys.argv[1]
if path.endswith(".zip"):
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
else:
    with tarfile.open(path) as archive:
        names = archive.getnames()
for name in names:
    while name.startswith("./"):
        name = name[2:]
    name = name.rstrip("/")
    if name and name != ".":
        print(name)
PY
}

rd_check_archive_layout() {
    local archive="$1" platform="$2" members expected entry top failed=0
    if [[ ! -s "$archive" ]]; then
        echo "!! $archive is missing or empty" >&2
        return 1
    fi
    members="$(rd_archive_members "$archive")" || return 1
    expected="$(rd_archive_entries "$platform")" || return 1
    while IFS= read -r entry; do
        if ! grep -qxF -- "$entry" <<< "$members"; then
            echo "!! $archive lacks $entry at its top level" >&2
            failed=1
        fi
    done <<< "$expected"
    if ! grep -qE '^plugins/[^/]+\.rdplug$' <<< "$members"; then
        echo "!! $archive carries no plugins/*.rdplug" >&2
        failed=1
    fi
    while IFS= read -r top; do
        [[ -z "$top" ]] && continue
        case "$top" in
            plugins|UNVERIFIED.txt) ;;
            *)
                if ! grep -qxF -- "$top" <<< "$expected"; then
                    echo "!! $archive carries $top, which no release archive does" >&2
                    failed=1
                fi
                ;;
        esac
    done < <(cut -d/ -f1 <<< "$members" | sort -u)
    if grep -E '^plugins/' <<< "$members" | grep -qvE '^plugins/[^/]+\.rdplug$'; then
        echo "!! $archive carries something under plugins/ that is not a signed package" >&2
        failed=1
    fi
    [[ "$failed" -eq 0 ]]
}
