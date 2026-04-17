import json
import tempfile
import unittest
from pathlib import Path

from rhythm_chip_controller.service import ChipControllerService, FakeChipBackend


class ChipControllerServiceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.backend = FakeChipBackend()
        self.service = ChipControllerService(self.backend)
        self.tmpdir = tempfile.TemporaryDirectory()
        self.storage_path = Path(self.tmpdir.name) / "chip" / "controller-storage.json"

    def tearDown(self) -> None:
        self.tmpdir.cleanup()

    def _handle(self, method: str, params=None, request_id: int = 1):
        envelope = {"id": request_id, "method": method}
        if params is not None:
            envelope["params"] = params
        return self.service.handle(envelope)

    def test_commission_round_trip_persists_and_lists_devices(self) -> None:
        init = self._handle(
            "init_controller",
            {
                "fabric_id": "default",
                "storage_path": str(self.storage_path),
                "ble_controller": 2,
            },
        )
        self.assertEqual(init["status"], "ok")
        self.assertEqual(init["result"]["fabric_id"], "default")

        commission = self._handle(
            "commission_light",
            {
                "setup_payload": "MT:TESTPAYLOAD",
                "node_id": 101,
                "network": "wifi",
                "rendezvous": "auto",
                "wifi_credentials": {"ssid": "Rhythm", "password": "secret"},
            },
            request_id=2,
        )
        self.assertEqual(commission["status"], "ok")
        self.assertEqual(commission["result"]["device"]["node_id"], 101)

        listed = self._handle("list_devices", request_id=3)
        self.assertEqual(listed["status"], "ok")
        self.assertEqual(len(listed["result"]["devices"]), 1)
        self.assertEqual(listed["result"]["devices"][0]["node_id"], 101)

        devices_path = self.storage_path.parent / "devices.json"
        persisted = json.loads(devices_path.read_text(encoding="utf-8"))
        self.assertEqual(persisted[0]["node_id"], 101)

    def test_control_methods_dispatch_to_backend(self) -> None:
        self._handle(
            "init_controller",
            {"fabric_id": "default", "storage_path": str(self.storage_path)},
        )
        self._handle(
            "commission_light",
            {
                "setup_payload": "manual-code",
                "node_id": 7,
                "network": "wifi",
                "rendezvous": "ble",
                "wifi_credentials": {"ssid": "Rhythm", "password": "secret"},
            },
            request_id=2,
        )

        self._handle(
            "set_on_off",
            {"node_id": 7, "endpoint": 1, "on": True},
            request_id=3,
        )
        self._handle(
            "set_brightness",
            {"node_id": 7, "endpoint": 1, "level": 200, "transition_ms": 500},
            request_id=4,
        )
        self._handle(
            "set_color_temperature",
            {"node_id": 7, "endpoint": 1, "kelvin": 3000, "transition_ms": 1000},
            request_id=5,
        )
        self._handle(
            "set_xy",
            {"node_id": 7, "endpoint": 1, "x": 0.4, "y": 0.3, "transition_ms": 250},
            request_id=6,
        )
        read = self._handle(
            "read_on_off",
            {"node_id": 7, "endpoint": 1},
            request_id=7,
        )

        self.assertEqual(read["status"], "ok")
        self.assertTrue(read["result"]["on"])
        self.assertEqual(
            [name for name, _args in self.backend.operations],
            [
                "commission_light",
                "set_on_off",
                "set_brightness",
                "set_color_temperature",
                "set_xy",
                "read_on_off",
            ],
        )

    def test_decommission_updates_persistent_cache(self) -> None:
        self._handle(
            "init_controller",
            {"fabric_id": "default", "storage_path": str(self.storage_path)},
        )
        self._handle(
            "commission_light",
            {
                "setup_payload": "manual-code",
                "node_id": 22,
                "network": "wifi",
                "rendezvous": "on_network",
                "wifi_credentials": {"ssid": "Rhythm", "password": "secret"},
            },
            request_id=2,
        )

        response = self._handle(
            "decommission_device",
            {"node_id": 22, "force": True},
            request_id=3,
        )
        self.assertEqual(response["status"], "ok")

        listed = self._handle("list_devices", request_id=4)
        self.assertEqual(listed["result"]["devices"], [])

    def test_errors_are_reported_in_rpc_shape(self) -> None:
        self._handle(
            "init_controller",
            {"fabric_id": "default", "storage_path": str(self.storage_path)},
        )
        response = self._handle(
            "commission_light",
            {
                "setup_payload": "bad-code",
                "node_id": 1,
                "network": "wifi",
                "rendezvous": "auto",
                "wifi_credentials": {"ssid": "Rhythm", "password": "secret"},
            },
            request_id=2,
        )
        self.assertEqual(response["status"], "error")
        self.assertIn("invalid setup payload", response["message"])


if __name__ == "__main__":
    unittest.main()
