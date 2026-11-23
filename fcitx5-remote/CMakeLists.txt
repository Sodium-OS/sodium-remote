# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
cmake_minimum_required(VERSION 3.21)
project(fcitx5-remote VERSION 0.1.0 LANGUAGES CXX)

# The input method engine that remoted drives over D-Bus while a Remote session is on
# See remote/docs/reference/ime-and-dnd.md for the interface.

find_package(Fcitx5Core 5.1 REQUIRED)
find_package(Fcitx5Module REQUIRED COMPONENTS DBus)
include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")
include(GNUInstallDirs)

add_library(remote MODULE src/remote.cpp src/compositor.cpp)
target_link_libraries(remote PRIVATE Fcitx5::Core Fcitx5::Module::DBus)
target_include_directories(remote PRIVATE src)

# fcitx5 finds addon descriptions under <data dir>/addon. The build tree gets its own
# copy so tests and the nested demo can point FCITX_DATA_DIRS at it without installing.
configure_file(src/remote-addon.conf.in "${CMAKE_BINARY_DIR}/data/addon/remote.conf" @ONLY)

install(TARGETS remote DESTINATION "${CMAKE_INSTALL_LIBDIR}/fcitx5")
install(FILES "${CMAKE_BINARY_DIR}/data/addon/remote.conf"
        DESTINATION "${CMAKE_INSTALL_DATADIR}/fcitx5/addon")

option(BUILD_TESTING "Build the headless test" ON)
if (BUILD_TESTING)
    enable_testing()
    find_package(Fcitx5Module REQUIRED COMPONENTS TestFrontend TestIM)
    add_executable(testremote test/testremote.cpp)
    target_link_libraries(testremote PRIVATE Fcitx5::Core Fcitx5::Module::TestFrontend
                          Fcitx5::Module::TestIM)
    target_include_directories(testremote PRIVATE src)
    target_compile_definitions(testremote PRIVATE
        REMOTE_BUILD_DIR="${CMAKE_BINARY_DIR}"
        REMOTE_TEST_DATA_DIR="${CMAKE_CURRENT_SOURCE_DIR}/test/data")
    add_dependencies(testremote remote)
    add_test(NAME testremote COMMAND testremote)
endif()
