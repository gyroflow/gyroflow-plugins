import QtQuick
import org.shotcut.qml

Metadata {
    type: Metadata.Filter
    name: qsTr("Gyroflow")
    keywords: qsTr("stabilization smooth shake action camera gyro") + " gyroflow #rgba"
    mlt_service: "frei0r.gyroflow"
    objectName: "gyroflow"
    qml: "ui.qml"
    isClipOnly: true
    allowMultiple: false
}
