# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote for Windows (SodiumRemote.exe): the Qt app of ../../../app cross-built with mingw, as a project of its own. Configured and built by
# remote/packaging/windows/stage-app.sh, with the toolchain file beside it.
cmake_minimum_required(VERSION 3.25)
project(sodium-remote-windows LANGUAGES C CXX RC)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)
get_filename_component(REMOTE_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)
# Sodium UI is built and staged by build-sodiumui.sh; the app only needs to know where its source is.
set(SODIUMUI_DIR "${REMOTE_ROOT}/sodium-ui")

find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui Qml Quick QuickControls2 Network)
qt_standard_project_setup(REQUIRES 6.8)
add_subdirectory(${REMOTE_ROOT}/app remote)
