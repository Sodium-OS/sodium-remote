// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// image://sodiumicon/<logical size>/<aarrggbb or ->/<url-encoded name or absolute path>
//
// Finds an icon in the SodiumOS icon theme (~/.local/share/icons/SodiumOS,
// then every XDG data dir), renders it at the requested pixel size, and
// recolours symbolic icons (their fill is currentColor / KDE ColorScheme-Text)
// to the given color. Names it doesn't have fall back to QIcon::fromTheme,
// which walks the system themes (breeze-dark, Adwaita, hicolor).
//
// image://sodiumicon/<logical size>/app[-light]/<name> is an app's icon: ours when we draw it,
// otherwise the app's own icon put on our tile (docs/decisions/icons.md G15).
// SodiumUI's Icon.qml builds these URLs; use that, not the URL format.
#pragma once

#include <QQuickImageProvider>

class IconProvider : public QQuickImageProvider
{
public:
    IconProvider();
    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override;
};
