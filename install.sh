#!/bin/sh
# Install cxvm, the static x86_64 Linux binary of a GitHub release.
#
#   curl -fsSL https://github.com/AndreaRicchi/cxvm/releases/latest/download/install.sh | sh
#   curl -fsSL … | sh -s -- --version v0.1.0 --bin-dir ~/bin
#
# The archive is checked against its sha256 before cxvm is put in
# $XDG_BIN_HOME, else ~/.local/bin. Nothing runs as root and no shell startup
# file is touched.
#
# Options, also read from the environment:
#   --version <tag>   CXVM_VERSION       the release to install (default: the latest)
#   --bin-dir <dir>   CXVM_BIN_DIR       where to put cxvm
#                     CXVM_RELEASES_URL  where the releases are, for mirrors
#                                        (default: https://github.com/AndreaRicchi/cxvm/releases)

set -eu

ARCHIVE=cxvm-x86_64-unknown-linux-musl.tar.gz
ISSUES=https://github.com/AndreaRicchi/cxvm/issues

die() {
    printf 'error: %s\n' "$1" >&2
    if [ -n "${2:-}" ]; then
        printf 'hint: %s\n' "$2" >&2
    fi
    exit 1
}

note() {
    printf 'note: %s\n' "$1" >&2
}

usage() {
    sed -n '2,15s/^# \{0,1\}//p' "$0" 2>/dev/null ||
        echo "usage: install.sh [--version <tag>] [--bin-dir <dir>]"
}

need() {
    command -v "$1" >/dev/null 2>&1 ||
        die "this script needs '$1', which is not on PATH" "install $2 and run it again"
}

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --proto-redir =https --retry 3 -o "$2" "$1"
    else
        wget -q -O "$2" "$1"
    fi
}

main() {
    releases=${CXVM_RELEASES_URL:-https://github.com/AndreaRicchi/cxvm/releases}
    version=${CXVM_VERSION:-}
    bin_dir=${CXVM_BIN_DIR:-}

    while [ $# -gt 0 ]; do
        case $1 in
            --version)
                [ $# -ge 2 ] || die "--version needs a release tag" "e.g. --version v0.1.0"
                version=$2
                shift 2
                ;;
            --bin-dir)
                [ $# -ge 2 ] || die "--bin-dir needs a directory" "e.g. --bin-dir ~/bin"
                bin_dir=$2
                shift 2
                ;;
            --version=*) version=${1#*=}; shift ;;
            --bin-dir=*) bin_dir=${1#*=}; shift ;;
            -h | --help) usage; exit 0 ;;
            *) die "unknown argument '$1'" "run 'sh install.sh --help' for the options" ;;
        esac
    done

    os=$(uname -s)
    arch=$(uname -m)
    if [ "$os" != Linux ] || [ "$arch" != x86_64 ]; then
        die "cxvm runs on x86_64 Linux only, and this is $os $arch" \
            "there is no build of cxvm for this host"
    fi

    if [ -z "$bin_dir" ]; then
        case ${XDG_BIN_HOME:-} in
            /*) bin_dir=$XDG_BIN_HOME ;;
            *)
                [ -n "${HOME:-}" ] || die "HOME is not set" "choose a directory with --bin-dir <dir>"
                bin_dir=$HOME/.local/bin
                ;;
        esac
    fi
    case $bin_dir in
        /*) ;;
        *) bin_dir=$PWD/$bin_dir ;;
    esac
    bin_dir=${bin_dir%/}

    command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 ||
        die "this script needs curl or wget, and neither is on PATH" \
            "install curl (e.g. 'sudo apt install curl') and run it again"
    need sha256sum coreutils
    need tar tar
    need gzip gzip
    need mktemp coreutils

    case $version in
        '') url=$releases/latest/download ;;
        v*) url=$releases/download/$version ;;
        *) version=v$version; url=$releases/download/$version ;;
    esac

    tmp=$(mktemp -d)
    partial=
    trap 'rm -rf "$tmp"; [ -z "$partial" ] || rm -f "$partial"' EXIT
    trap 'exit 1' HUP INT TERM

    echo "Downloading $url/$ARCHIVE" >&2
    for file in "$ARCHIVE" "$ARCHIVE.sha256"; do
        fetch "$url/$file" "$tmp/$file" ||
            die "could not download $url/$file" \
                "check that ${version:-the latest release} exists at $releases, and your network or proxy"
    done

    expected=$(cut -d ' ' -f 1 <"$tmp/$ARCHIVE.sha256")
    actual=$(sha256sum "$tmp/$ARCHIVE" | cut -d ' ' -f 1)
    case $expected in
        *[!0-9a-f]* | '') die "$ARCHIVE.sha256 holds no sha256" "report it at $ISSUES" ;;
    esac
    [ "$actual" = "$expected" ] ||
        die "$ARCHIVE does not match its sha256 (expected $expected, got $actual)" \
            "run this again; if it keeps failing, report it at $ISSUES"

    tar -xzf "$tmp/$ARCHIVE" -C "$tmp" ||
        die "could not unpack $ARCHIVE" "report it at $ISSUES"
    new=$tmp/${ARCHIVE%.tar.gz}/cxvm
    installed=$("$new" --version) ||
        die "the downloaded cxvm does not run" "report it at $ISSUES, with the output above"

    if ! mkdir -p "$bin_dir" 2>/dev/null || ! [ -w "$bin_dir" ]; then
        die "cannot write to $bin_dir" \
            "choose a directory of your own with --bin-dir <dir>; this script does not use sudo"
    fi
    # Copied next to the target and renamed, so that cxvm is never half written.
    partial=$bin_dir/.cxvm.install.$$
    if ! { cp "$new" "$partial" && chmod 755 "$partial" && mv -f "$partial" "$bin_dir/cxvm"; }; then
        die "could not write $bin_dir/cxvm" "check the free space and permissions of $bin_dir"
    fi
    partial=

    echo "Installed $installed in $bin_dir/cxvm" >&2

    case :${PATH:-}: in
        *:$bin_dir:* | *:$bin_dir/:*)
            found=$(command -v cxvm || true)
            if [ "$found" != "$bin_dir/cxvm" ]; then
                note "another cxvm comes first on PATH: $found"
            fi
            ;;
        *)
            note "$bin_dir is not on PATH; add it in your shell's startup file:"
            # shellcheck disable=SC2016 # $PATH is printed, not expanded
            printf '  export PATH="%s:$PATH"  # bash, zsh\n' "$bin_dir" >&2
            printf '  fish_add_path %s  # fish\n' "$bin_dir" >&2
            ;;
    esac
    echo "Next: 'cxvm install gcc@15' and 'cxvm use gcc@15' in a project," \
        "and 'cxvm completions --help' for shell completions." >&2
}

main "$@"
