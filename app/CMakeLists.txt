# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote's Qt app (app ID org.sodiumos.Remote; binary sodium-remote, SodiumRemote.exe on Windows): the same code on
# Linux and Windows. The engine is Rust (client-qt, a static library through a C ABI); the app is Qt/QML
# on Sodium UI.
#
# Three ways to build it:
#  - on its own (the public repository):   cmake -S app -B build        (Sodium UI from ../sodium-ui, built here)
#  - SodiumOS's apps/ project:             adds this directory with SODIUMUI_DIR set to SodiumOS's ui/ (already built)
#  - Windows, cross-built:                 packaging/windows/app/ (its own project around this directory)
# SODIUMUI_DIR: where Sodium UI is. Left unset it is the vendored ../sodium-ui, which is built as part of this project;
# a directory given by the caller is used as it is (it must already hold a built SodiumUI/ module).
if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
    cmake_minimum_required(VERSION 3.25)
    project(sodium-remote LANGUAGES C CXX)
    set(CMAKE_CXX_STANDARD 20)
    set(CMAKE_CXX_STANDARD_REQUIRED ON)
    set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui Qml Quick QuickControls2)
    qt_standard_project_setup(REQUIRES 6.8)
    enable_testing()
endif()
get_filename_component(REMOTE_RUST_DIR "${CMAKE_CURRENT_SOURCE_DIR}/.." ABSOLUTE)
include("${REMOTE_RUST_DIR}/cmake/RemoteApp.cmake")
# REMOTE_LINUX_HOST: "This computer" on generic Linux (remote-qt's linux-host feature: host-desktop's portal backend in this
# process). On for the standalone build, which is the one with no SODIUMUI_DIR from a caller; off in SodiumOS's apps/ build,
# which hosts with remoted. -DREMOTE_LINUX_HOST=ON|OFF overrides either.
if(NOT DEFINED REMOTE_LINUX_HOST)
    if(SODIUMUI_DIR)
        set(REMOTE_LINUX_HOST OFF)
    else()
        set(REMOTE_LINUX_HOST ON)
    endif()
endif()
if(NOT SODIUMUI_DIR)
    set(SODIUMUI_DIR "${REMOTE_RUST_DIR}/sodium-ui")
    add_subdirectory("${SODIUMUI_DIR}" sodium-ui)
    set(SODIUMUI_IMPORT_DIR "${SODIUMUI_IMPORT_ROOT}")
else()
    set(SODIUMUI_IMPORT_DIR "${SODIUMUI_DIR}")
endif()
if(NOT CMAKE_SYSTEM_NAME STREQUAL "Linux" AND NOT WIN32)
    return()
endif()
# Windows ("Sodium Remote", SodiumRemote.exe): cross-built by remote/packaging/windows/app/ (its own CMake project
# around this directory), with the engine already built by cargo (remote/packaging/windows/stage-app.sh).
if(NOT WIN32)
    find_program(CARGO cargo)
    if(NOT CARGO)
        message(STATUS "remote: cargo not found, skipping")
        return()
    endif()
endif()
find_package(Qt6 REQUIRED COMPONENTS Network GuiPrivate)

# The engine: cargo builds libremote_qt.a in remote/target (the workspace's own target dir; it is gitignored).
if(WIN32)
    set(REMOTE_ENGINE_LIB "${REMOTE_RUST_DIR}/target/x86_64-pc-windows-gnu/release/libremote_qt.a")
    add_library(remote_engine_lib STATIC IMPORTED GLOBAL)
    set_target_properties(remote_engine_lib PROPERTIES IMPORTED_LOCATION "${REMOTE_ENGINE_LIB}")
    # `cargo rustc --release --target x86_64-pc-windows-gnu -p remote-qt -- --print native-static-libs`, plus what Qt-side
    # code here uses. libwindows.0.52.0.a (windows-targets' import library) is copied next to the deps by stage-app.sh.
    set(REMOTE_ENGINE_NATIVE avcodec avutil swscale opus winmm windows.0.52.0 bcrypt advapi32 kernel32 ntdll userenv
        ws2_32 dbghelp user32 shell32 ole32 gdi32 dwmapi)
else()
    set(REMOTE_ENGINE_LIB "${REMOTE_RUST_DIR}/target/release/libremote_qt.a")
    set(REMOTE_ENGINE_FEATURES "")
    if(REMOTE_LINUX_HOST)
        set(REMOTE_ENGINE_FEATURES --features linux-host)
    endif()
    add_custom_target(remote_engine
        COMMAND ${CARGO} build --release -p remote-qt ${REMOTE_ENGINE_FEATURES}
        WORKING_DIRECTORY "${REMOTE_RUST_DIR}"
        BYPRODUCTS "${REMOTE_ENGINE_LIB}"
        COMMENT "Building the Remote engine (cargo, remote-qt)"
        USES_TERMINAL)
    add_library(remote_engine_lib STATIC IMPORTED GLOBAL)
    set_target_properties(remote_engine_lib PROPERTIES IMPORTED_LOCATION "${REMOTE_ENGINE_LIB}")
    add_dependencies(remote_engine_lib remote_engine)
    # `cargo rustc -p remote-qt -- --print native-static-libs`.
    set(REMOTE_ENGINE_NATIVE wayland-egl wayland-client EGL avcodec avutil pipewire-0.3 opus m dl pthread rt util gcc_s)
    if(REMOTE_LINUX_HOST)
        # What the host adds (`cargo rustc -p remote-qt --features linux-host -- --print native-static-libs`); OpenH264 is loaded at run time.
        list(APPEND REMOTE_ENGINE_NATIVE avfilter swscale xkbcommon)
    endif()
endif()

# The models and the controller, a library so the tests link them without the window.
set(REMOTE_APP_SOURCES src/models.h src/models.cpp src/controller.h src/controller.cpp src/remote_qt.h src/firewallrules.h src/firewallrules.cpp)
if(NOT WIN32)
    list(APPEND REMOTE_APP_SOURCES src/linkhandoff.h src/linkhandoff.cpp)
endif()
if(WIN32)
    list(APPEND REMOTE_APP_SOURCES src/streamframe.h src/streamframe.cpp src/hostcontroller.h src/hostcontroller.cpp
        src/tray.h src/tray.cpp src/winapp.h src/winapp.cpp)
elseif(REMOTE_LINUX_HOST)
    list(APPEND REMOTE_APP_SOURCES src/hostcontroller.h src/hostcontroller.cpp)
endif()
# The release (the workspace version, remote/Cargo.toml) and where the source tree is (the notices file is found there in a build tree).
file(STRINGS "${REMOTE_RUST_DIR}/Cargo.toml" _ver_line REGEX "^version = \"" LIMIT_COUNT 1)
string(REGEX REPLACE "^version = \"([^\"]*)\".*" "\\1" REMOTE_VERSION "${_ver_line}")
add_library(remote_core_app STATIC ${REMOTE_APP_SOURCES})
target_compile_definitions(remote_core_app PUBLIC REMOTE_VERSION="${REMOTE_VERSION}" REMOTE_SOURCE_DIR="${REMOTE_RUST_DIR}")
target_include_directories(remote_core_app PUBLIC src)
if(WIN32)
    if(DEFINED ENV{WIN_DEPS})
        set(_win_deps "$ENV{WIN_DEPS}")
    else()
        set(_win_deps "${REMOTE_RUST_DIR}/build/win-deps")
    endif()
    target_link_directories(remote_core_app PUBLIC "${_win_deps}/lib")
endif()
if(REMOTE_LINUX_HOST)
    find_package(Qt6 REQUIRED COMPONENTS DBus)
    target_compile_definitions(remote_core_app PUBLIC REMOTE_LINUX_HOST=1)
    target_link_libraries(remote_core_app PUBLIC Qt6::DBus)
endif()
target_link_libraries(remote_core_app PUBLIC Qt6::Core Qt6::Gui Qt6::GuiPrivate Qt6::Network Qt6::Qml Qt6::Quick remote_engine_lib ${REMOTE_ENGINE_NATIVE})
set_target_properties(remote_core_app PROPERTIES AUTOMOC ON POSITION_INDEPENDENT_CODE ON)

set_source_files_properties(qml/Fmt.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)
set(REMOTE_WIN_QML "")
set(REMOTE_WIN_RESOURCES "")
set(REMOTE_WIN_RESOURCES windows/icon.svg) # the app icon: the Windows window bar (Main.qml) and About, everywhere
if(WIN32)
    # The Windows-only screens (new files; the shared QML is not touched): This computer, the tray menu, the "being controlled" notice.
    set(REMOTE_WIN_QML qml/ThisComputerPage.qml qml/TrayMenu.qml qml/ControlledNotice.qml)
elseif(REMOTE_LINUX_HOST)
    set(REMOTE_WIN_QML qml/ThisComputerPage.qml) # generic Linux: the same page, with the desktop's permission in place of the firewall
endif()
remote_app(remote ID org.sodiumos.Remote
    SOURCES src/main.cpp
    QML
        qml/Main.qml
        qml/Fmt.qml
        qml/ComputerCard.qml
        qml/ComputersPage.qml
        qml/AddSheet.qml
        qml/CodeField.qml
        qml/SettingsSheet.qml
        qml/FailureCard.qml
        qml/ConnectingCard.qml
        qml/StreamWindow.qml
        qml/StreamBar.qml
        qml/OsPill.qml
        qml/StatusPill.qml
        qml/AboutPage.qml
        qml/NoticesSheet.qml
        qml/ServerPage.qml
        qml/ServerSheet.qml
        qml/ServerFiles.qml
        ${REMOTE_WIN_QML}
    RESOURCES ${REMOTE_WIN_RESOURCES})
target_link_libraries(remote PRIVATE remote_core_app)
# The app's own symbolic icons, in its resources at :/icons/sodium-remote (main.cpp lists that as a fallback icon search path).
file(GLOB_RECURSE REMOTE_ICON_FILES CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/data/icons/sodium-remote/*")
qt_add_resources(remote remote_icons PREFIX "/icons/sodium-remote" BASE "${CMAKE_CURRENT_SOURCE_DIR}/data/icons/sodium-remote"
    FILES ${REMOTE_ICON_FILES})
if(NOT WIN32)
    set_target_properties(remote PROPERTIES OUTPUT_NAME sodium-remote)
endif()
if(TARGET sodiumui_module)
    add_dependencies(remote sodiumui_module)
endif()
if(WIN32)
    set_target_properties(remote PROPERTIES OUTPUT_NAME SodiumRemote)
    target_link_options(remote PRIVATE -mwindows)
    target_sources(remote PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/windows/SodiumRemote.rc")
endif()

find_package(Qt6 COMPONENTS Test)
if(Qt6Test_FOUND AND NOT WIN32)
    qt_add_executable(tst_remote tests/tst_remote.cpp)
    target_link_libraries(tst_remote PRIVATE remote_core_app Qt6::Test)
    set_target_properties(tst_remote PROPERTIES AUTOMOC ON)
    add_test(NAME remote COMMAND tst_remote)
    set_tests_properties(remote PROPERTIES TIMEOUT 60 ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endif()
