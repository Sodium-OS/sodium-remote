#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# cargo with env.sh's Windows settings, for callers that can't source it:
#   remote/packaging/windows/cargo.sh build --target x86_64-pc-windows-gnu -p host-desktop
# Runs from remote/ (the workspace). WIN_DEPS may point at another worktree's build/win-deps.
set -e
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=env.sh
. "$here/env.sh"
cd "$here/../.."
exec cargo "$@"
