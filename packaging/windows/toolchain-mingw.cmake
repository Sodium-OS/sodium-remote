# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# CMake toolchain for Windows x64 from Linux: Arch's mingw-w64 GCC (UCRT), Qt 6.12.1's official
# mingw x64 build (fetch-qt.sh) and the dependencies of build-icu.sh / build-deps.sh.
#
#   cmake -S ui -B build -DCMAKE_TOOLCHAIN_FILE=remote/packaging/windows/toolchain-mingw.cmake ...
#
# Qt's tools that run during the build (moc, rcc, qsb, ...) come from the Linux Qt, which must be
# the same version: QT_HOST_PATH. build-sodiumui.sh passes it.
get_filename_component(_win_dir "${CMAKE_CURRENT_LIST_FILE}" DIRECTORY)
get_filename_component(_remote "${_win_dir}/../.." ABSOLUTE)

set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)

set(_triplet x86_64-w64-mingw32)
set(CMAKE_C_COMPILER ${_triplet}-gcc)
set(CMAKE_CXX_COMPILER ${_triplet}-g++)
set(CMAKE_RC_COMPILER ${_triplet}-windres)
set(CMAKE_AR ${_triplet}-ar)
set(CMAKE_RANLIB ${_triplet}-ranlib)

if(NOT DEFINED ENV{QT_WIN_ROOT})
    set(_qt_root "${_remote}/build/qt-win/6.12.1/mingw_64")
else()
    set(_qt_root "$ENV{QT_WIN_ROOT}")
endif()
if(NOT DEFINED ENV{WIN_DEPS})
    set(_deps "${_remote}/build/win-deps")
else()
    set(_deps "$ENV{WIN_DEPS}")
endif()

set(CMAKE_PREFIX_PATH "${_qt_root};${_deps}")
set(CMAKE_FIND_ROOT_PATH "${_qt_root};${_deps};/usr/${_triplet}")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# Qt's cross-compiling support: the host Qt (same version) provides the build tools.
if(NOT DEFINED QT_HOST_PATH)
    set(QT_HOST_PATH /usr CACHE PATH "Linux Qt 6.12.1 for the host tools")
    set(QT_HOST_PATH_CMAKE_DIR /usr/lib/cmake CACHE PATH "")
endif()
