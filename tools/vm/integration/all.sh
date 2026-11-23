#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Every Windows screen in en, de and ja, dark and light: run-computers.sh and run-this.sh for each pair.
#   all.sh HOSTDIR OUTDIR      (HOSTDIR: a running `spikes/remote-e2e/run.sh HOSTDIR --host-only`; VM lock held)
here=$(cd "$(dirname "$0")" && pwd)
for lang in en_US de_DE ja_JP; do
    for theme in dark light; do
        "$here/run-computers.sh" "$1" "$2" "$lang" "$theme"
        "$here/run-this.sh" "$2" "$lang" "$theme"
    done
done
