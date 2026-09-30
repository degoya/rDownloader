#!/usr/bin/env bash
#
# The signed apt and dnf repositories of the deb and rpm packages (RD-180-10), as static files for
# GitHub Pages: new packages from <incoming> go into the repository tree <site>, which keeps what
# earlier releases put there, and the indexes are written and signed again.
#
# Usage:
#   scripts/package-repo.sh <incoming> <site> [--keep N] [--key KEY] [--base-url URL] [--refresh]
#
#   <incoming>   a directory of .deb and .rpm files (release asset names do not matter: every file
#                is filed under the name its own metadata gives it; .src.rpm is left out)
#   <site>       the repository tree, created when missing, updated in place:
#                  deb/pool/main/<l>/<name>/<name>_<version>_<arch>.deb
#                  deb/dists/stable/{Release,InRelease,Release.gpg}
#                  deb/dists/stable/main/binary-{amd64,arm64,…}/Packages{,.gz}
#                  rpm/packages/<name>-<version>-<release>.<arch>.rpm   (signed with rpmsign)
#                  rpm/repodata/…, rpm/repodata/repomd.xml.asc
#                  rdownloader.asc (the public key), rdownloader.sources (deb822, Signed-By),
#                  rdownloader.repo (dnf, zypper), README.md, index.html, .nojekyll
#   --keep N     versions kept per package and architecture, default 2: the newest N, older ones
#                are removed from the pool. GitHub Pages publishes at most 1 GB; a release brings
#                four packages of about 55 MB (deb and rpm, two architectures), so two versions
#                are about 440 MB
#   --key KEY    the GPG key that signs (fingerprint or user id); default: the only secret key in
#                $GNUPGHOME, and more or fewer than one is an error
#   --base-url   where <site> is served, default https://degoya.github.io/rdownloader-packages;
#                the .sources, the .repo and the README point at it
#   --refresh    write and sign the indexes even when no package came or went (a new key)
#
# A package whose file is already in the pool is kept as it is — a re-run of a release neither
# replaces a published file nor signs an rpm twice — and with nothing added or removed the signed
# indexes stay untouched. The deb half needs dpkg-deb and apt-ftparchive (apt-utils), the rpm half
# rpm, rpmsign and createrepo_c (Ubuntu: rpm, createrepo-c); each half runs when it has packages.
# The key is used without a passphrase; in CI it is a secret imported into a keyring of the run
# (docs/development.md, *apt and dnf repositories*).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Bash 5.2 would put the matched text in place of a `&` in a substitution's replacement.
shopt -u patsub_replacement 2> /dev/null || true
shopt -s nullglob

usage() {
    echo "usage: $0 <incoming> <site> [--keep N] [--key KEY] [--base-url URL] [--refresh]" >&2
    exit 2
}

[[ $# -ge 2 ]] || usage
incoming="$1"
site="$2"
shift 2
keep=2
key=""
base_url="https://degoya.github.io/rdownloader-packages"
refresh=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --keep) [[ $# -ge 2 ]] || usage; keep="$2"; shift 2 ;;
        --key) [[ $# -ge 2 ]] || usage; key="$2"; shift 2 ;;
        --base-url) [[ $# -ge 2 ]] || usage; base_url="$2"; shift 2 ;;
        --refresh) refresh=1; shift ;;
        *) usage ;;
    esac
done
base_url="${base_url%/}"

[[ -d "$incoming" ]] || { echo "error: $incoming is not a directory" >&2; exit 1; }
[[ "$keep" =~ ^[1-9][0-9]*$ ]] || { echo "error: --keep wants a count of at least 1, not '$keep'" >&2; exit 1; }
[[ "$base_url" =~ ^(https?|file):// ]] || { echo "error: '$base_url' is no http(s):// or file:// URL" >&2; exit 1; }

need() {
    local tool
    for tool in "$@"; do
        command -v "$tool" > /dev/null || { echo "error: $tool is not installed (${need_hint})" >&2; exit 1; }
    done
}
need_hint="GnuPG"
need gpg

# The signing key, by fingerprint from here on.
secret_fingerprints() {
    gpg --batch --with-colons --list-secret-keys "$@" 2> /dev/null \
        | awk -F: '$1 == "sec" { want = 1; next } want && $1 == "fpr" { print $10; want = 0 }'
}
mapfile -t fingerprints < <(if [[ -n "$key" ]]; then secret_fingerprints "$key"; else secret_fingerprints; fi)
if [[ "${#fingerprints[@]}" -ne 1 ]]; then
    echo "error: ${#fingerprints[@]} secret keys match '${key:-any}' in ${GNUPGHOME:-~/.gnupg}; name exactly one with --key" >&2
    exit 1
fi
fingerprint="${fingerprints[0]}"

debs=("$incoming"/*.deb)
rpms=()
for file in "$incoming"/*.rpm; do
    [[ "$file" == *.src.rpm ]] || rpms+=("$file")
done
mkdir -p "$site"
site="$(cd "$site" && pwd)"

# Prints the pool files beyond the newest $keep of each package and architecture, from
# tab-separated lines "<name> <arch> <version> <path>" on stdin.
beyond_keep() {
    sort -t $'\t' -k1,1 -k2,2 -k3,3Vr | awk -F '\t' -v keep="$keep" '
        { group = $1 "\t" $2; count[group]++; if (count[group] > keep) print $4 }'
}

sign_detached() { gpg --batch --yes --local-user "$fingerprint" --digest-algo SHA512 --armor --detach-sign "$@"; }

deb_changed=0
if [[ "${#debs[@]}" -gt 0 || -d "$site/deb/pool" ]]; then
    need_hint="Debian/Ubuntu: dpkg, apt-utils"
    need dpkg-deb apt-ftparchive
    for file in "${debs[@]}"; do
        read -r name arch version < <(dpkg-deb --show --showformat='${Package} ${Architecture} ${Version}\n' "$file")
        [[ -n "${version:-}" ]] || { echo "error: $file is no Debian package" >&2; exit 1; }
        target="$site/deb/pool/main/${name:0:1}/${name}/${name}_${version#*:}_${arch}.deb"
        if [[ -e "$target" ]]; then
            echo "kept    ${target#"$site"/} (already in the pool)"
            continue
        fi
        mkdir -p "$(dirname "$target")"
        cp "$file" "$target"
        echo "added   ${target#"$site"/}"
        deb_changed=1
    done
    while read -r stale; do
        rm -f "$stale"
        echo "removed ${stale#"$site"/} (beyond the newest $keep)"
        deb_changed=1
    done < <(for file in "$site"/deb/pool/main/*/*/*.deb; do
        printf '%s\t%s\n' "$(dpkg-deb --show --showformat='${Package}\t${Architecture}\t${Version}' "$file")" "$file"
    done | beyond_keep)
    find "$site/deb/pool" -mindepth 1 -type d -empty -delete 2> /dev/null || true

    if [[ "$deb_changed" -eq 1 || "$refresh" -eq 1 || ! -f "$site/deb/dists/stable/InRelease" ]]; then
        # amd64 and arm64 always get an index, so apt on either finds one; any other
        # architecture in the pool is added. Architecture `all` belongs in every index.
        mapfile -t architectures < <( { printf 'amd64\narm64\n'
            for file in "$site"/deb/pool/main/*/*/*.deb; do
                arch="${file##*_}"
                echo "${arch%.deb}"
            done; } | grep -vx all | sort -u)
        dists="$site/deb/dists/stable"
        rm -rf "$dists"
        for arch in "${architectures[@]}"; do
            mkdir -p "$dists/main/binary-$arch"
            (cd "$site/deb" && apt-ftparchive --arch "$arch" packages pool) > "$dists/main/binary-$arch/Packages"
            gzip -9n --keep "$dists/main/binary-$arch/Packages"
        done
        (cd "$site/deb" && apt-ftparchive \
            -o APT::FTPArchive::Release::Origin=rDownloader \
            -o APT::FTPArchive::Release::Label=rDownloader \
            -o APT::FTPArchive::Release::Suite=stable \
            -o APT::FTPArchive::Release::Codename=stable \
            -o APT::FTPArchive::Release::Components=main \
            -o "APT::FTPArchive::Release::Architectures=${architectures[*]}" \
            -o "APT::FTPArchive::Release::Description=rDownloader packages" \
            release dists/stable) > "$site/deb/Release.new"
        mv "$site/deb/Release.new" "$dists/Release"
        gpg --batch --yes --local-user "$fingerprint" --digest-algo SHA512 --clearsign \
            --output "$dists/InRelease" "$dists/Release"
        sign_detached --output "$dists/Release.gpg" "$dists/Release"
        echo "signed  deb/dists/stable/InRelease and Release.gpg (${architectures[*]})"
    else
        echo "deb: nothing added or removed, the signed index stays"
    fi
fi

rpm_changed=0
if [[ "${#rpms[@]}" -gt 0 || -d "$site/rpm/packages" ]]; then
    need_hint="Ubuntu: rpm, createrepo-c; Fedora: rpm-sign, createrepo_c"
    need rpm rpmsign createrepo_c
    for file in "${rpms[@]}"; do
        canonical="$(rpm -qp --nosignature --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}.rpm' "$file")"
        target="$site/rpm/packages/$canonical"
        if [[ -e "$target" ]]; then
            echo "kept    ${target#"$site"/} (already in the repository)"
            continue
        fi
        mkdir -p "$site/rpm/packages"
        cp "$file" "$target"
        # A v4 signature over header and payload; the release asset itself stays unsigned.
        rpmsign --define "_gpg_name $fingerprint" --define "_gpg_sign_cmd_extra_args --batch" \
            --addsign "$target" > /dev/null
        echo "added   ${target#"$site"/} (signed)"
        rpm_changed=1
    done
    while read -r stale; do
        rm -f "$stale"
        echo "removed ${stale#"$site"/} (beyond the newest $keep)"
        rpm_changed=1
    done < <(for file in "$site"/rpm/packages/*.rpm; do
        printf '%s\t%s\n' "$(rpm -qp --nosignature --qf '%{NAME}\t%{ARCH}\t%{VERSION}-%{RELEASE}' "$file")" "$file"
    done | beyond_keep)

    if [[ "$rpm_changed" -eq 1 || "$refresh" -eq 1 || ! -f "$site/rpm/repodata/repomd.xml.asc" ]]; then
        # gzip, not createrepo_c's newer zstd default: older dnf and zypper read only the former.
        createrepo_c --quiet --checksum sha256 --general-compress-type gz "$site/rpm"
        sign_detached --output "$site/rpm/repodata/repomd.xml.asc" "$site/rpm/repodata/repomd.xml"
        echo "signed  rpm/repodata/repomd.xml.asc"
    else
        echo "rpm: nothing added or removed, the signed metadata stays"
    fi
fi

if [[ ! -d "$site/deb" && ! -d "$site/rpm" ]]; then
    echo "error: $incoming has no .deb or .rpm, and $site holds no repository yet" >&2
    exit 1
fi

# The files a user adds to their system, the key they trust and the pages that explain both.
gpg --batch --armor --export "$fingerprint" > "$site/rdownloader.asc"
values=("BASE_URL=$base_url" "FINGERPRINT=$fingerprint" "KEEP=$keep")
render() {
    local template="$ROOT/packaging/repository/$1" output="$site/${1%.in}" content pair
    content="$(< "$template")"
    for pair in "${values[@]}"; do
        content="${content//@${pair%%=*}@/${pair#*=}}"
    done
    if [[ "$content" =~ @[A-Z0-9_]+@ ]]; then
        echo "error: $template keeps ${BASH_REMATCH[0]} after rendering" >&2
        exit 1
    fi
    printf '%s\n' "$content" > "$output"
}
[[ -d "$site/deb" ]] && render rdownloader.sources.in
[[ -d "$site/rpm" ]] && render rdownloader.repo.in
render README.md.in
render index.html.in
: > "$site/.nojekyll"
echo "wrote   rdownloader.asc, the source files and the README for $base_url (key $fingerprint)"
