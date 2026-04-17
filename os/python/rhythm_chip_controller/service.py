"""Service layer for the local CHIP controller sidecar."""

from __future__ import annotations

import asyncio
import json
import math
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, Optional, Protocol


BASIC_INFORMATION_CLUSTER = 0x0028
DESCRIPTOR_CLUSTER = 0x001D
ON_OFF_CLUSTER = 0x0006
LEVEL_CONTROL_CLUSTER = 0x0008
COLOR_CONTROL_CLUSTER = 0x0300


def _json_error(message: str) -> Dict[str, Any]:
    return {"status": "error", "message": message}


def _transition_tenths(transition_ms: Optional[int]) -> int:
    if not transition_ms:
        return 0
    return max(0, int(transition_ms) // 100)


def _kelvin_to_mireds(kelvin: int) -> int:
    if kelvin <= 0:
        return 500
    return max(1, min(65279, 1_000_000 // kelvin))


def _xy_to_matter(value: float) -> int:
    return max(0, min(65535, int(round(float(value) * 65535.0))))


def _read_json(path: Path, default: Any) -> Any:
    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError:
        return default


def _write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = path.with_suffix(path.suffix + ".tmp")
    with temp_path.open("w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
    os.replace(temp_path, path)


@dataclass
class CommissioningState:
    fabric_id: str
    storage_path: Path
    ble_controller: Optional[int]

    @property
    def devices_path(self) -> Path:
        return self.storage_path.parent / "devices.json"


class ControllerBackend(Protocol):
    def init_controller(self, state: CommissioningState) -> Dict[str, Any]:
        ...

    def commission_light(self, request: Dict[str, Any]) -> Dict[str, Any]:
        ...

    def probe_light(self, node_id: int) -> Dict[str, Any]:
        ...

    def decommission_device(self, node_id: int, force: bool) -> None:
        ...

    def set_on_off(self, node_id: int, endpoint: int, on: bool) -> None:
        ...

    def set_brightness(
        self, node_id: int, endpoint: int, level: int, transition_ms: Optional[int]
    ) -> None:
        ...

    def set_color_temperature(
        self, node_id: int, endpoint: int, kelvin: int, transition_ms: Optional[int]
    ) -> None:
        ...

    def set_xy(
        self,
        node_id: int,
        endpoint: int,
        x: float,
        y: float,
        transition_ms: Optional[int],
    ) -> None:
        ...

    def read_on_off(self, node_id: int, endpoint: int) -> bool:
        ...


class DeviceStore:
    def __init__(self) -> None:
        self._path: Optional[Path] = None
        self._devices: Dict[int, Dict[str, Any]] = {}

    def configure(self, path: Path) -> None:
        self._path = path
        raw_devices = _read_json(path, [])
        self._devices = {int(device["node_id"]): dict(device) for device in raw_devices}

    def list_devices(self) -> list[Dict[str, Any]]:
        return [
            {
                "node_id": device["node_id"],
                "vendor_name": device.get("vendor_name", ""),
                "product_name": device.get("product_name", ""),
                "reachable": bool(device.get("reachable", True)),
            }
            for device in sorted(self._devices.values(), key=lambda device: int(device["node_id"]))
        ]

    def upsert(self, device: Dict[str, Any]) -> None:
        self._devices[int(device["node_id"])] = dict(device)
        self._flush()

    def remove(self, node_id: int) -> None:
        self._devices.pop(int(node_id), None)
        self._flush()

    def _flush(self) -> None:
        if self._path is None:
            return
        payload = [
            self._devices[node_id]
            for node_id in sorted(self._devices.keys())
        ]
        _write_json(self._path, payload)


class ChipControllerService:
    def __init__(self, backend: ControllerBackend) -> None:
        self._backend = backend
        self._device_store = DeviceStore()
        self._state: Optional[CommissioningState] = None

    def handle(self, envelope: Dict[str, Any]) -> Dict[str, Any]:
        request_id = envelope.get("id")
        method = envelope.get("method")
        params = envelope.get("params")

        try:
            result = self._dispatch(method, params)
            return {"id": request_id, "status": "ok", "result": result}
        except Exception as exc:  # pragma: no cover - exercised by tests through responses
            return {"id": request_id, **_json_error(str(exc))}

    def _dispatch(self, method: str, params: Any) -> Any:
        if method == "init_controller":
            assert isinstance(params, dict)
            state = CommissioningState(
                fabric_id=str(params["fabric_id"]),
                storage_path=Path(str(params["storage_path"])),
                ble_controller=params.get("ble_controller"),
            )
            self._state = state
            self._device_store.configure(state.devices_path)
            return self._backend.init_controller(state)

        if self._state is None:
            raise RuntimeError("Controller not initialized")

        if method == "commission_light":
            assert isinstance(params, dict)
            device = self._backend.commission_light(params)
            self._device_store.upsert(device)
            return {"device": device}
        if method == "list_devices":
            return {"devices": self._device_store.list_devices()}
        if method == "probe_light":
            assert isinstance(params, dict)
            device = self._backend.probe_light(int(params["node_id"]))
            self._device_store.upsert(device)
            return {"device": device}
        if method == "decommission_device":
            assert isinstance(params, dict)
            node_id = int(params["node_id"])
            force = bool(params.get("force", False))
            self._backend.decommission_device(node_id, force)
            self._device_store.remove(node_id)
            return {}
        if method == "set_on_off":
            assert isinstance(params, dict)
            self._backend.set_on_off(
                int(params["node_id"]),
                int(params["endpoint"]),
                bool(params["on"]),
            )
            return {}
        if method == "set_brightness":
            assert isinstance(params, dict)
            self._backend.set_brightness(
                int(params["node_id"]),
                int(params["endpoint"]),
                int(params["level"]),
                params.get("transition_ms"),
            )
            return {}
        if method == "set_color_temperature":
            assert isinstance(params, dict)
            self._backend.set_color_temperature(
                int(params["node_id"]),
                int(params["endpoint"]),
                int(params["kelvin"]),
                params.get("transition_ms"),
            )
            return {}
        if method == "set_xy":
            assert isinstance(params, dict)
            self._backend.set_xy(
                int(params["node_id"]),
                int(params["endpoint"]),
                float(params["x"]),
                float(params["y"]),
                params.get("transition_ms"),
            )
            return {}
        if method == "read_on_off":
            assert isinstance(params, dict)
            on = self._backend.read_on_off(int(params["node_id"]), int(params["endpoint"]))
            return {"on": on}

        raise RuntimeError(f"Unsupported method '{method}'")


class OfficialChipBackend:
    """Best-effort wrapper around the official Python CHIP controller bindings."""

    def __init__(self) -> None:
        self._state: Optional[CommissioningState] = None
        self._controller: Any = None
        self._bindings: Optional[tuple[Any, Any, Any]] = None

    def init_controller(self, state: CommissioningState) -> Dict[str, Any]:
        self._state = state
        state.storage_path.parent.mkdir(parents=True, exist_ok=True)
        return {"fabric_id": state.fabric_id}

    def commission_light(self, request: Dict[str, Any]) -> Dict[str, Any]:
        controller = self._ensure_controller()
        wifi = request["wifi_credentials"]
        setup_payload = str(request["setup_payload"])
        node_id = int(request["node_id"])
        rendezvous = str(request.get("rendezvous", "auto"))

        controller.SetWiFiCredentials(str(wifi["ssid"]), str(wifi["password"]))
        discovery_type = self._discovery_type(rendezvous)

        try:
            effective_node_id = int(
                self._run(controller.CommissionWithCode(setup_payload, node_id, discovery_type))
            )
        except Exception:
            effective_node_id = int(
                self._fallback_commission_ble_wifi(
                    controller=controller,
                    setup_payload=setup_payload,
                    node_id=node_id,
                    wifi=wifi,
                )
            )

        return self.probe_light(effective_node_id)

    def probe_light(self, node_id: int) -> Dict[str, Any]:
        controller = self._ensure_controller()
        _, clusters, cluster_objects = self._bindings_or_raise()

        wildcard = self._run(controller.ReadAttribute(node_id, [()]))
        light_endpoint = self._detect_light_endpoint(wildcard, clusters)
        # Explicit Descriptor read keeps endpoint detection aligned with the spec.
        self._read_attribute(node_id, light_endpoint, DESCRIPTOR_CLUSTER, 0x0001)

        vendor_name = str(self._read_attribute(node_id, 0, BASIC_INFORMATION_CLUSTER, 0x0001) or "")
        product_name = str(self._read_attribute(node_id, 0, BASIC_INFORMATION_CLUSTER, 0x0002) or "")
        vendor_id = int(self._read_attribute(node_id, 0, BASIC_INFORMATION_CLUSTER, 0x0004) or 0)
        product_id = int(self._read_attribute(node_id, 0, BASIC_INFORMATION_CLUSTER, 0x0005) or 0)
        serial_number = self._read_attribute(node_id, 0, BASIC_INFORMATION_CLUSTER, 0x000F)

        color_modes: list[str] = []
        capabilities_raw = self._read_attribute(node_id, light_endpoint, COLOR_CONTROL_CLUSTER, 0x400A)
        if isinstance(capabilities_raw, int):
            if capabilities_raw & 0x01:
                color_modes.append("hue_saturation")
            if capabilities_raw & 0x08:
                color_modes.append("xy")
            if capabilities_raw & 0x10:
                color_modes.append("color_temperature")

        if not color_modes:
            color_mode = self._read_attribute(node_id, light_endpoint, COLOR_CONTROL_CLUSTER, 0x0008)
            if color_mode == 0:
                color_modes.append("hue_saturation")
            elif color_mode == 1:
                color_modes.append("xy")
            elif color_mode is not None:
                color_modes.append("color_temperature")

        min_mireds = self._read_attribute(node_id, light_endpoint, COLOR_CONTROL_CLUSTER, 0x400C)
        max_mireds = self._read_attribute(node_id, light_endpoint, COLOR_CONTROL_CLUSTER, 0x400D)
        min_kelvin = int(1_000_000 / max_mireds) if isinstance(max_mireds, int) and max_mireds > 0 else None
        max_kelvin = int(1_000_000 / min_mireds) if isinstance(min_mireds, int) and min_mireds > 0 else None

        return {
            "node_id": int(node_id),
            "vendor_name": vendor_name,
            "product_name": product_name,
            "vendor_id": vendor_id,
            "product_id": product_id,
            "serial_number": None if not serial_number else str(serial_number),
            "light_endpoint": int(light_endpoint),
            "color_modes": color_modes,
            "min_kelvin": min_kelvin,
            "max_kelvin": max_kelvin,
        }

    def decommission_device(self, node_id: int, force: bool) -> None:
        controller = self._ensure_controller()
        try:
            self._run(controller.UnpairDevice(node_id))
        except Exception:
            if not force:
                raise

    def set_on_off(self, node_id: int, endpoint: int, on: bool) -> None:
        controller = self._ensure_controller()
        clusters, _, _ = self._bindings_or_raise()
        command = clusters.OnOff.Commands.On() if on else clusters.OnOff.Commands.Off()
        self._run(controller.SendCommand(node_id, endpoint, command))

    def set_brightness(
        self, node_id: int, endpoint: int, level: int, transition_ms: Optional[int]
    ) -> None:
        controller = self._ensure_controller()
        clusters, _, _ = self._bindings_or_raise()
        command = clusters.LevelControl.Commands.MoveToLevelWithOnOff(
            level=int(level),
            transitionTime=_transition_tenths(transition_ms),
            optionsMask=0,
            optionsOverride=0,
        )
        self._run(controller.SendCommand(node_id, endpoint, command))

    def set_color_temperature(
        self, node_id: int, endpoint: int, kelvin: int, transition_ms: Optional[int]
    ) -> None:
        controller = self._ensure_controller()
        clusters, _, _ = self._bindings_or_raise()
        command = clusters.ColorControl.Commands.MoveToColorTemperature(
            colorTemperatureMireds=_kelvin_to_mireds(int(kelvin)),
            transitionTime=_transition_tenths(transition_ms),
            optionsMask=0,
            optionsOverride=0,
        )
        self._run(controller.SendCommand(node_id, endpoint, command))

    def set_xy(
        self,
        node_id: int,
        endpoint: int,
        x: float,
        y: float,
        transition_ms: Optional[int],
    ) -> None:
        controller = self._ensure_controller()
        clusters, _, _ = self._bindings_or_raise()
        command = clusters.ColorControl.Commands.MoveToColor(
            colorX=_xy_to_matter(x),
            colorY=_xy_to_matter(y),
            transitionTime=_transition_tenths(transition_ms),
            optionsMask=0,
            optionsOverride=0,
        )
        self._run(controller.SendCommand(node_id, endpoint, command))

    def read_on_off(self, node_id: int, endpoint: int) -> bool:
        value = self._read_attribute(node_id, endpoint, ON_OFF_CLUSTER, 0x0000)
        return bool(value)

    def _ensure_controller(self) -> Any:
        if self._controller is not None:
            return self._controller
        if self._state is None:
            raise RuntimeError("Controller not initialized")

        controller_module, _clusters, _cluster_objects = self._bindings_or_raise()
        controller_cls = getattr(controller_module, "ChipDeviceController", None)
        if controller_cls is None:
            raise RuntimeError(
                "Python CHIP controller bindings are installed, but ChipDeviceController was not found"
            )

        candidates = [
            {
                "storagePath": str(self._state.storage_path),
                "bleController": self._state.ble_controller,
            },
            {
                "storagePath": str(self._state.storage_path),
            },
            {},
        ]
        last_error: Optional[Exception] = None
        for kwargs in candidates:
            try:
                clean_kwargs = {
                    key: value for key, value in kwargs.items() if value is not None
                }
                self._controller = controller_cls(**clean_kwargs)
                return self._controller
            except TypeError as exc:
                last_error = exc
                continue

        raise RuntimeError(
            "Unable to construct ChipDeviceController from the installed Matter Python bindings"
        ) from last_error

    def _bindings_or_raise(self) -> tuple[Any, Any, Any]:
        if self._bindings is None:
            self._bindings = self._load_bindings()
        return self._bindings

    def _load_bindings(self) -> tuple[Any, Any, Any]:
        attempts = [
            ("matter.ChipDeviceCtrl", "matter.clusters"),
            ("chip.ChipDeviceCtrl", "chip.clusters"),
        ]
        for controller_name, clusters_name in attempts:
            try:
                controller_module = __import__(controller_name, fromlist=["*"])
                clusters_module = __import__(clusters_name, fromlist=["*"])
                cluster_objects = getattr(clusters_module, "ClusterObjects", None)
                if cluster_objects is None:
                    cluster_objects = __import__(
                        f"{clusters_name}.ClusterObjects", fromlist=["*"]
                    )
                return controller_module, clusters_module, cluster_objects
            except ImportError:
                continue

        raise RuntimeError(
            "Official Matter Python bindings are not installed. Build and activate the connectedhomeip Python controller environment first."
        )

    def _run(self, awaitable: Any) -> Any:
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return asyncio.run(awaitable)
        if not loop.is_running():
            return loop.run_until_complete(awaitable)
        temp_loop = asyncio.new_event_loop()
        try:
            return temp_loop.run_until_complete(awaitable)
        finally:
            temp_loop.close()

    def _discovery_type(self, rendezvous: str) -> Any:
        controller_module, _clusters, _cluster_objects = self._bindings_or_raise()
        discovery_type = getattr(controller_module, "DiscoveryType", None)
        if discovery_type is None:
            return 0

        candidates = {
            "auto": ["DISCOVERY_ALL", "DISCOVERY_TYPE_ALL"],
            "ble": ["DISCOVERY_BLE", "DISCOVERY_ALL", "DISCOVERY_TYPE_BLE"],
            "on_network": ["DISCOVERY_NETWORK_ONLY", "DISCOVERY_IP", "DISCOVERY_ALL"],
        }.get(rendezvous, ["DISCOVERY_ALL"])
        for name in candidates:
            if hasattr(discovery_type, name):
                return getattr(discovery_type, name)
        return 0

    def _fallback_commission_ble_wifi(
        self, controller: Any, setup_payload: str, node_id: int, wifi: Dict[str, Any]
    ) -> int:
        payload = self._decode_setup_payload(setup_payload)
        if payload is None:
            raise RuntimeError(
                "CommissionWithCode failed and no supported setup-payload decoder was available for BLE Wi-Fi fallback"
            )
        return int(
            self._run(
                controller.CommissionBleWiFi(
                    payload["discriminator"],
                    payload["passcode"],
                    node_id,
                    str(wifi["ssid"]),
                    str(wifi["password"]),
                )
            )
        )

    def _decode_setup_payload(self, setup_payload: str) -> Optional[Dict[str, int]]:
        module_names = ["matter.setup_payload", "chip.setup_payload"]
        for module_name in module_names:
            try:
                module = __import__(module_name, fromlist=["*"])
            except ImportError:
                continue
            parser_cls = getattr(module, "SetupPayload", None)
            if parser_cls is None:
                continue
            parser = parser_cls()
            methods = [
                "ParseQrCode",
                "ParseManualCode",
                "parse_qr_code",
                "parse_manual_code",
            ]
            for method_name in methods:
                method = getattr(parser, method_name, None)
                if method is None:
                    continue
                try:
                    method(setup_payload)
                except Exception:
                    continue
                discriminator = (
                    getattr(parser, "long_discriminator", None)
                    or getattr(parser, "discriminator", None)
                    or getattr(parser, "setup_discriminator", None)
                )
                passcode = (
                    getattr(parser, "passcode", None)
                    or getattr(parser, "setup_passcode", None)
                    or getattr(parser, "setup_pin_code", None)
                )
                if discriminator is not None and passcode is not None:
                    return {
                        "discriminator": int(discriminator),
                        "passcode": int(passcode),
                    }
        return None

    def _detect_light_endpoint(self, wildcard: Any, clusters: Any) -> int:
        if not isinstance(wildcard, dict):
            return 1
        for endpoint, endpoint_clusters in wildcard.items():
            try:
                endpoint_id = int(endpoint)
            except (TypeError, ValueError):
                continue
            if endpoint_id == 0 or not isinstance(endpoint_clusters, dict):
                continue
            if clusters.OnOff in endpoint_clusters or clusters.LevelControl in endpoint_clusters:
                return endpoint_id
        return 1

    def _read_attribute(self, node_id: int, endpoint: int, cluster_id: int, attribute_id: int) -> Any:
        controller = self._ensure_controller()
        _, _clusters, cluster_objects = self._bindings_or_raise()
        attribute_class = cluster_objects.ALL_ATTRIBUTES[cluster_id][attribute_id]
        response = self._run(controller.ReadAttribute(node_id, [(endpoint, attribute_class)]))

        try:
            cluster_class = cluster_objects.ALL_CLUSTERS[cluster_id]
            endpoint_view = response[endpoint]
            cluster_view = endpoint_view[cluster_class]
            if attribute_class in cluster_view:
                return cluster_view[attribute_class]
        except Exception:
            pass

        if isinstance(response, dict):
            endpoint_view = response.get(endpoint)
            if isinstance(endpoint_view, dict):
                for cluster_view in endpoint_view.values():
                    if isinstance(cluster_view, dict) and attribute_class in cluster_view:
                        return cluster_view[attribute_class]

        return None


class FakeChipBackend:
    """In-memory fake backend used by unit tests."""

    def __init__(self) -> None:
        self.init_calls: list[CommissioningState] = []
        self.devices: Dict[int, Dict[str, Any]] = {}
        self.on_off_state: Dict[int, bool] = {}
        self.operations: list[tuple[str, tuple[Any, ...]]] = []

    def init_controller(self, state: CommissioningState) -> Dict[str, Any]:
        self.init_calls.append(state)
        return {"fabric_id": state.fabric_id}

    def commission_light(self, request: Dict[str, Any]) -> Dict[str, Any]:
        if str(request["setup_payload"]).startswith("bad"):
            raise RuntimeError("invalid setup payload")
        device = {
            "node_id": int(request["node_id"]),
            "vendor_name": "Test Vendor",
            "product_name": "Test Bulb",
            "vendor_id": 1,
            "product_id": 2,
            "serial_number": None,
            "light_endpoint": 1,
            "color_modes": ["xy", "color_temperature"],
            "min_kelvin": 2200,
            "max_kelvin": 6500,
        }
        self.devices[device["node_id"]] = dict(device)
        self.operations.append(("commission_light", (request["setup_payload"],)))
        return device

    def probe_light(self, node_id: int) -> Dict[str, Any]:
        self.operations.append(("probe_light", (node_id,)))
        return dict(self.devices[node_id])

    def decommission_device(self, node_id: int, force: bool) -> None:
        self.operations.append(("decommission_device", (node_id, force)))
        self.devices.pop(node_id, None)
        self.on_off_state.pop(node_id, None)

    def set_on_off(self, node_id: int, endpoint: int, on: bool) -> None:
        self.operations.append(("set_on_off", (node_id, endpoint, on)))
        self.on_off_state[node_id] = on

    def set_brightness(
        self, node_id: int, endpoint: int, level: int, transition_ms: Optional[int]
    ) -> None:
        self.operations.append(("set_brightness", (node_id, endpoint, level, transition_ms)))
        self.on_off_state[node_id] = level > 0

    def set_color_temperature(
        self, node_id: int, endpoint: int, kelvin: int, transition_ms: Optional[int]
    ) -> None:
        self.operations.append(
            ("set_color_temperature", (node_id, endpoint, kelvin, transition_ms))
        )

    def set_xy(
        self,
        node_id: int,
        endpoint: int,
        x: float,
        y: float,
        transition_ms: Optional[int],
    ) -> None:
        self.operations.append(("set_xy", (node_id, endpoint, x, y, transition_ms)))

    def read_on_off(self, node_id: int, endpoint: int) -> bool:
        self.operations.append(("read_on_off", (node_id, endpoint)))
        return bool(self.on_off_state.get(node_id, False))

