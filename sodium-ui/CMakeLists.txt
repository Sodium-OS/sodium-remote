# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium UI's native part and QML, vendored for Sodium Remote's app (derived from ui/CMakeLists.txt; MANIFEST, UPSTREAM).
#
# Always the portable configuration: no D-Bus, Wayland, xkbcommon, librsvg or GLib; Qt SVG draws every icon. The build
# assembles a ready import directory, <build>/SodiumUI/ (the qmldir, the QML, theme.default.json, the plugin), so
# `engine.addImportPath(<build>)` is all an app needs. Added by app/CMakeLists.txt (SODIUMUI_DIR defaults to this folder).
cmake_minimum_required(VERSION 3.21)
if(NOT PROJECT_NAME)
    project(SodiumUI LANGUAGES CXX)
endif()

set(CMAKE_AUTOMOC ON)
find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui Qml Quick Svg ShaderTools Network)
# ICU: locale date skeletons (LocaleFormats); Qt itself is built on it.
find_package(ICU REQUIRED COMPONENTS uc i18n)

set(SODIUMUI_IMPORT_ROOT "${CMAKE_CURRENT_BINARY_DIR}" CACHE INTERNAL "where <root>/SodiumUI/ is assembled")
set(_mod "${CMAKE_CURRENT_BINARY_DIR}/SodiumUI")

add_library(sodiumuinative MODULE
    native/plugin.cpp
    native/themestore.cpp native/themestore.h
    native/iconprovider.cpp native/iconprovider.h
    native/sodiumwmwindow.cpp native/sodiumwmwindow.h
    native/localeformats.cpp native/localeformats.h
    native/calendarinfo.cpp native/calendarinfo.h
    native/i18n.cpp native/i18n.h
    native/desktopindex.cpp native/desktopindex.h
    native/i18naudit.cpp native/i18naudit.h
    native/pseudolocale.cpp native/pseudolocale.h
    native/sort.cpp native/sort.h
    native/textbreaks.cpp native/textbreaks.h
    native/textmatch.cpp native/textmatch.h)
target_compile_features(sodiumuinative PRIVATE cxx_std_17)
target_compile_definitions(sodiumuinative PRIVATE SODIUMUI_PORTABLE=1)
target_link_libraries(sodiumuinative PRIVATE Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick Qt6::Svg Qt6::Network ICU::uc ICU::i18n)
if(WIN32)
    target_sources(sodiumuinative PRIVATE native/portable/windowchrome.cpp native/portable/windowchrome.h)
endif()
if(NOT WIN32)
    target_link_libraries(sodiumuinative PRIVATE ${CMAKE_DL_LIBS})
endif()
set_target_properties(sodiumuinative PROPERTIES
    PREFIX ""
    LIBRARY_OUTPUT_DIRECTORY "${_mod}"
    RUNTIME_OUTPUT_DIRECTORY "${_mod}")
if(NOT WIN32)
    set_target_properties(sodiumuinative PROPERTIES PREFIX "lib")
endif()

# Shaders for Glass.qml and Icon.qml, compiled into the plugin: qrc:/sodiumui/shaders/*.frag.qsb
qt_add_shaders(sodiumuinative "sodiumui_shaders"
    PREFIX "/sodiumui"
    BASE SodiumUI
    FILES SodiumUI/shaders/glass.frag SodiumUI/shaders/tint.frag)
# The in-window blur variant of the glass shader (Glass.backdrop): qrc:/sodiumui/shaders/glassblur.frag.qsb
qt_add_shaders(sodiumuinative "sodiumui_shaders_backdrop"
    PREFIX "/sodiumui"
    BASE SodiumUI
    DEFINES BACKDROP=1
    FILES SodiumUI/shaders/glass.frag
    OUTPUTS shaders/glassblur.frag.qsb)

# The QML files, the qmldir and the defaults, copied next to the plugin.
file(GLOB _qml CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/SodiumUI/*.qml" "${CMAKE_CURRENT_SOURCE_DIR}/SodiumUI/*.js"
    "${CMAKE_CURRENT_SOURCE_DIR}/SodiumUI/qmldir" "${CMAKE_CURRENT_SOURCE_DIR}/SodiumUI/theme.default.json")
set(_copied "")
foreach(f IN LISTS _qml)
    get_filename_component(n "${f}" NAME)
    add_custom_command(OUTPUT "${_mod}/${n}" COMMAND ${CMAKE_COMMAND} -E copy_if_different "${f}" "${_mod}/${n}" DEPENDS "${f}")
    list(APPEND _copied "${_mod}/${n}")
endforeach()
add_custom_target(sodiumui_module ALL DEPENDS ${_copied} sodiumuinative)
