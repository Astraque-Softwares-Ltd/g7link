// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// Stands in for the Rust DeviceList in QML tests: no phones.
ListModel {
    function deviceNameAt(index) { return "" }
    function rowOf(device) { return -1 }
}
