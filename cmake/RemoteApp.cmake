# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# remote_app(<target> ID <app id> [QT <extra Qt components...>] SOURCES ... QML ... RESOURCES ...)
#
# The app's executable and its QML module (named after the target), the .desktop file check, and where Sodium UI is:
# SODIUMUI_IMPORT_DIR, the directory that holds SodiumUI/ (the build tree), compiled in for runs from the build
# directory. Inlined from SodiumOS's sodiumos_app so this repository builds with nothing outside it.
function(remote_app target)
    cmake_parse_arguments(APP "" "ID" "QT;SOURCES;QML;RESOURCES" ${ARGN})
    if(NOT APP_ID)
        message(FATAL_ERROR "remote_app(${target}): ID is required")
    endif()
    if(NOT EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/${APP_ID}.desktop")
        message(FATAL_ERROR "remote_app(${target}): ${APP_ID}.desktop is missing")
    endif()
    foreach(component IN LISTS APP_QT)
        find_package(Qt6 REQUIRED COMPONENTS ${component})
    endforeach()
    qt_add_executable(${target} ${APP_SOURCES})
    qt_add_qml_module(${target} URI ${target} VERSION 1.0 QML_FILES ${APP_QML} RESOURCES ${APP_RESOURCES})
    target_compile_definitions(${target} PRIVATE
        SODIUMUI_IMPORT_DIR="${SODIUMUI_IMPORT_DIR}"
        REMOTE_APP_ID="${APP_ID}")
    target_link_libraries(${target} PRIVATE Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick Qt6::QuickControls2)
    foreach(component IN LISTS APP_QT)
        target_link_libraries(${target} PRIVATE Qt6::${component})
    endforeach()
endfunction()
