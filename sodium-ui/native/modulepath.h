// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The directory the SodiumUI plugin was loaded from: the module directory (the qmldir's), since
// the plugin sits next to it (libsodiumuinative.so on Linux, sodiumuinative.dll in the portable
// build). Defined in the header so each translation unit that needs it shares one lookup.
#pragma once

#include <QFileInfo>
#include <QString>

#ifdef Q_OS_WIN
#  include <windows.h>
#else
#  include <dlfcn.h>
#endif

inline QString sodiumuiModuleDir()
{
#ifdef Q_OS_WIN
    HMODULE module = nullptr;
    if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(&sodiumuiModuleDir), &module)) {
        wchar_t path[32768];
        const DWORD n = GetModuleFileNameW(module, path, DWORD(sizeof(path) / sizeof(path[0])));
        if (n > 0 && n < sizeof(path) / sizeof(path[0]))
            return QFileInfo(QString::fromWCharArray(path, int(n))).absolutePath();
    }
    return {};
#else
    Dl_info info{};
    if (dladdr(reinterpret_cast<void *>(&sodiumuiModuleDir), &info) && info.dli_fname)
        return QFileInfo(QString::fromLocal8Bit(info.dli_fname)).absolutePath();
    return {};
#endif
}
