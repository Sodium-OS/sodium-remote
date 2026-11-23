#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote's translation pipeline: two catalogs, `remote` (the Qt app) and `sodiumui` (the vendored Sodium UI), in the
# ten UI languages (docs/translating.md). A trimmed copy of SodiumOS's scripts/i18n.sh and its tools (i18n/tools/).
#
#   i18n/i18n.sh update [catalog ...]   lupdate6 the catalogs' sources into i18n/ts/<catalog>_<lang>.ts (nine languages, plus
#                                       en_US with plural forms only), keeping translations and review states
#   i18n/i18n.sh reconcile              lupdate in place keeping obsolete messages, then re-attach vanished translations
#   i18n/i18n.sh release [--strict] [--optional] [--only CATALOG] [--out DIR]
#                                       lrelease6 every .ts into build/i18n/qm/<catalog>_<lang>.qm (or DIR)
#   i18n/i18n.sh generate               the .desktop copy with every translation, into build/i18n/desktop
#   i18n/i18n.sh lint [args]            the unwrapped-string lint (tools/lint.py; --strict, --summary)
#   i18n/i18n.sh check [--strict]       lint + the .ts checks (placeholders, plurals, punctuation, glossary)
#
# update, reconcile and release need qt6-tools (lupdate6, lrelease6); generate, lint and check never do.
# Output goes under build/ (never /tmp, which is a RAM disk).
set -euo pipefail

here=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
root=$(cd "$here/.." && pwd)        # the repository root (SodiumOS: remote/)
cd "$root"
py=i18n/tools
ts_dir=i18n/ts
out=build/i18n
langs="en_GB en_AU zh_CN zh_TW ja_JP ko_KR es_ES fr_FR de_DE"
source_lang=en_US

# The catalogs and where their strings come from (.qml, .js, .cpp, .h files; tests and fixtures left out). The app's also
# reads the .desktop file's strings through a generated source.
declare -A sources=(
    [remote]="app generated:desktop-entry.cpp"
    [sodiumui]="sodium-ui/SodiumUI sodium-ui/native"
)
catalog_order="remote sodiumui"

die() { echo "i18n.sh: $*" >&2; exit 1; }

find_tool() {
    local name=$1 c
    for c in "${name}6" "${name}-qt6" "/usr/lib/qt6/bin/$name" "$name"; do
        if command -v "$c" >/dev/null 2>&1 && "$c" -version 2>&1 | grep -q "Qt 6\|version 6\|6\.[0-9]"; then echo "$c"; return 0; fi
    done
    return 1
}
need_tool() { find_tool "$1" || die "$1 not found: install qt6-tools. Nothing was changed."; }

generate_sources() {
    mkdir -p "$out/src"
    python3 -I "$py/gen-desktop.py" extract "$out/src/desktop-entry.cpp"
}

source_list() {
    local cat=$1 list="$out/lst/$cat.lst" d
    mkdir -p "$out/lst"
    : >"$list"
    for d in ${sources[$cat]}; do
        if [[ $d == generated:* ]]; then
            echo "$root/$out/src/${d#generated:}" >>"$list"
        elif git rev-parse --git-dir >/dev/null 2>&1 && [[ -n $(git ls-files -- "$d" 2>/dev/null | head -1) ]]; then
            git ls-files -- "$d" | grep -E '\.(qml|js|cpp|h|hpp)$' | grep -Ev '(^|/)(tests?|fixtures|build[^/]*|gallery|data)/' | sed "s|^|$root/|" >>"$list" || true
        else
            find "$d" -type f \( -name '*.qml' -o -name '*.js' -o -name '*.cpp' -o -name '*.h' -o -name '*.hpp' \) \
                -not -path '*/tests/*' -not -path '*/build*/*' -not -path '*/data/*' | sort | sed "s|^|$root/|" >>"$list"
        fi
    done
    echo "$list"
}

cmd_update() {
    local lupdate cats=("$@") cat lang list before=$out/ts-before
    lupdate=$(need_tool lupdate)
    [[ ${#cats[@]} -eq 0 ]] && cats=($catalog_order)
    generate_sources
    mkdir -p "$ts_dir"
    rm -rf "$before"; mkdir -p "$before"; cp -a "$ts_dir/." "$before/"
    for cat in "${cats[@]}"; do
        [[ -n ${sources[$cat]+x} ]] || die "unknown catalog: $cat (known: $catalog_order)"
        list=$(source_list "$cat")
        [[ -s $list ]] || { echo "  $cat: no sources, skipped"; continue; }
        for lang in $langs; do
            "$lupdate" -silent -no-obsolete -locations absolute -source-language $source_lang -target-language "$lang" \
                "@$list" -ts "$ts_dir/${cat}_${lang}.ts"
        done
        "$lupdate" -silent -no-obsolete -locations absolute -pluralonly -source-language $source_lang -target-language $source_lang \
            "@$list" -ts "$ts_dir/${cat}_${source_lang}.ts"
        echo "  $cat: $(grep -c '<message' "$ts_dir/${cat}_de_DE.ts") messages"
    done
    python3 -I "$py/reconcile.py" --before "$before" --ts-dir "$ts_dir"
}

cmd_reconcile() {
    local lupdate cat lang list before="$out/ts-before"
    lupdate=$(need_tool lupdate)
    generate_sources
    rm -rf "$before"; mkdir -p "$ts_dir" "$before"; cp -a "$ts_dir/." "$before/"
    for cat in $catalog_order; do
        list=$(source_list "$cat")
        [[ -s $list ]] || continue
        for lang in $langs; do
            "$lupdate" -silent -locations absolute -source-language $source_lang -target-language "$lang" "@$list" -ts "$ts_dir/${cat}_${lang}.ts"
        done
        "$lupdate" -silent -locations absolute -pluralonly -source-language $source_lang -target-language $source_lang \
            "@$list" -ts "$ts_dir/${cat}_${source_lang}.ts"
    done
    python3 -I "$py/reconcile.py" --before "$before" --ts-dir "$ts_dir"
}

# The .qm files go to build/i18n/qm (what the app finds from a build directory and a SodiumOS checkout alike), or --out DIR
# (a package build: <prefix>/share/sodium-remote/translations, or translations/ beside the Windows program).
cmd_release() {
    local lrelease flags="" f dest="$out/qm" optional=0 only=""
    while [[ $# -gt 0 ]]; do
        case $1 in
        --strict) flags="-nounfinished" ;;
        --optional) optional=1 ;;
        --out) shift; dest=${1:?--out needs a directory} ;;
        --only) shift; only=${1:?--only needs a catalog} ;;
        *) die "release: unknown argument $1" ;;
        esac
        shift
    done
    if ! lrelease=$(find_tool lrelease); then
        if ((optional)); then echo "i18n.sh: lrelease6 not found (qt6-tools): no .qm catalogs built, the app shows its source text" >&2; return 0; fi
        die "lrelease not found: install qt6-tools. Nothing was changed."
    fi
    shopt -s nullglob
    local files=("$ts_dir"/*.ts) n=0
    [[ ${#files[@]} -gt 0 ]] || die "no catalogs in $ts_dir yet: run i18n/i18n.sh update first"
    mkdir -p "$dest"
    for f in "${files[@]}"; do
        [[ -z $only || $(basename "$f") == "${only}_"* ]] || continue
        "$lrelease" -silent $flags "$f" -qm "$dest/$(basename "${f%.ts}").qm"
        n=$((n + 1))
    done
    echo "  $n catalogs in $dest"
}

cmd_generate() {
    mkdir -p "$out"
    python3 -I "$py/gen-desktop.py" generate "$out/desktop" --ts-dir "$ts_dir"
    echo "  .desktop copy in $out/desktop"
}

cmd_lint() { python3 -I "$py/lint.py" "$@"; }

cmd_check() {
    local strict="" rc=0
    [[ ${1:-} == --strict ]] && strict="--strict"
    python3 -I "$py/lint.py" --summary $strict || rc=1
    python3 -I "$py/check.py" $strict --catalogs "${catalog_order// /,}" || rc=1
    return $rc
}

case "${1:-}" in
update) shift; cmd_update "$@" ;;
reconcile) shift; cmd_reconcile "$@" ;;
release) shift; cmd_release "$@" ;;
generate) shift; cmd_generate "$@" ;;
lint) shift; cmd_lint "$@" ;;
check) shift; cmd_check "$@" ;;
*) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; [[ -z ${1:-} || $1 == -h || $1 == --help ]] || exit 2 ;;
esac
