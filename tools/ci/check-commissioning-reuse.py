#!/usr/bin/env python3
"""Guard the shared commissioning owners; behavior is proved by their tests."""
import argparse
from pathlib import Path
import tempfile

APP = "app/flutter/rhythm_app/lib/"
SDK = "sdk/lib/src/api/"
RUST = "os/rust/"
RULES = {
    APP + "screens/hubs/matter_device_add_screen.dart": (
        ["DeviceCommissioningFlow", "_flow.begin()", "_flow.expectReceipt()", "Try from phone"],
        ["_phoneAttemptToReconcile", "_newPairingSessionId"],
    ),
    APP + "screens/hubs/ble_wifi_device_add_screen.dart": (
        ["DeviceCommissioningFlow", "_flow.begin()", "PhoneBleWifiServices.create", "Try from phone", "Try from Rhythm Box"],
        ["AylaPhoneBleWifiService", "Uuid().v4()}-$stage"],
    ),
    APP + "widgets/commissioning_wifi_dialog.dart": (
        ["widget.validateWifi(wifi)"], ["Ayla", "encodeWifi"],
    ),
    SDK + "rhythm_server_api.dart": (["readPairingReceipt(_dio, sessionId)"], []),
    SDK + "rhythm_matter_api.dart": (["readPairingReceipt(_dio, sessionId,"], []),
    "app/flutter/rhythm_app/ios/PhoneMatterRequestContext.swift": (
        ["enum PhoneMatterBridge", "static func responseEnvelope"], [],
    ),
    "app/flutter/rhythm_app/ios/Runner/AppDelegate.swift": (
        ["PhoneMatterBridge.appGroup"], ["static let appGroup", "static let responseKey", "baseURLKey", "authTokenKey", "createdAtKey"],
    ),
    "app/flutter/rhythm_app/ios/MatterCommissioningExtension/RequestHandler.swift": (
        ["PhoneMatterBridge.responseEnvelope"], ["static let appGroup", "static let responseKey"],
    ),
    RUST + "integrations/rhythm-matter/src/commissioning.rs": (
        ["provisioning::load_accessory_wifi_credentials(state)"],
        ["fn load_stored_commissioning_wifi_credentials", "fn load_platform_commissioning_wifi_credentials"],
    ),
    RUST + "integrations/rhythm-monster/src/hub.rs": (
        ["provisioning::load_accessory_wifi_credentials(state)"], [],
    ),
}


def check(root):
    errors = []
    for relative, (required, forbidden) in RULES.items():
        source = (root / relative).read_text()
        errors.extend(f"{relative}: missing shared owner {text}" for text in required if text not in source)
        errors.extend(f"{relative}: duplicated protocol/policy {text}" for text in forbidden if text in source)
    return errors


def self_test():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for relative, (required, _) in RULES.items():
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("\n".join(required))
        assert not check(root)
        for relative, (required, forbidden) in RULES.items():
            target = root / relative
            original = target.read_text()
            target.write_text(original.replace(required[0], "duplicated local implementation"))
            assert check(root), relative
            for anchor in forbidden:
                target.write_text(original + "\n" + anchor)
                assert check(root), (relative, anchor)
            target.write_text(original)
        assert not check(root)
    print("PASS: shared commissioning owner guard positive/negative fixtures")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, nargs="?", default=Path(__file__).resolve().parents[2])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
    else:
        failures = check(args.root)
        if failures:
            raise SystemExit("\n".join(failures))
        print("PASS: shared commissioning lifecycle, receipt reader, credential owner and protocol-neutral UI")
