// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import QtQuick
import QtQuick.Shapes
import QtQuick.Templates as T

// Busy spinner (the Phase 2 boards' process-working symbol): in a 16 px box,
// a 5.33 px-radius ring at 30% accent with a quarter arc in full accent,
// 1.33 px lines, turning once a second.
//
//     Spinner { running: loading }
//     Spinner { size: 24 }
T.BusyIndicator {
    id: control

    property real size: 16
    property color color: Theme.accentInk
    readonly property real unit: size / 16

    implicitWidth: size
    implicitHeight: size
    padding: 0
    Accessible.role: Accessible.Animation

    contentItem: Item {
        visible: control.running

        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer

            ShapePath {
                fillColor: "transparent"
                strokeColor: Qt.rgba(control.color.r, control.color.g, control.color.b, control.color.a * 0.3)
                strokeWidth: 1.333 * control.unit
                PathAngleArc {
                    centerX: control.size / 2
                    centerY: control.size / 2
                    radiusX: 5.333 * control.unit
                    radiusY: radiusX
                    startAngle: 0
                    sweepAngle: 360
                }
            }
            ShapePath {
                fillColor: "transparent"
                strokeColor: control.color
                strokeWidth: 1.333 * control.unit
                capStyle: ShapePath.RoundCap
                PathAngleArc {
                    centerX: control.size / 2
                    centerY: control.size / 2
                    radiusX: 5.333 * control.unit
                    radiusY: radiusX
                    startAngle: -90
                    sweepAngle: 90
                }
            }
        }

        RotationAnimator on rotation {
            running: control.running && control.visible
            loops: Animation.Infinite
            from: 0
            to: 360
            duration: 1000
        }
    }
}
