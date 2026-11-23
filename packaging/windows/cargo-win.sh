#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# cargo for x86_64-pc-windows-gnu with env.sh applied: cargo-win.sh build --release -p client-windows
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/env.sh"
cd "$here/../.."
sub=$1
shift
exec cargo "$sub" --target x86_64-pc-windows-gnu "$@"
