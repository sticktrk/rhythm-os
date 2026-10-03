#!/usr/bin/env python3
"""Exercise the SDK patch against supplied BluezEndpoint.cpp with real GLib.

This isolates the production callback and admission prefix, replacing GATT I/O
with counters. It is not a complete CHIP compilation or a hardware BLE test.
"""
import argparse
import os
from pathlib import Path
import shlex
import subprocess
import tempfile


def method(source, signature):
    start = source.index(signature)
    brace = source.index("{", start)
    depth = 0
    for end in range(brace, len(source)):
        depth += (source[end] == "{") - (source[end] == "}")
        if depth == 0:
            return source[start:end + 1]
    raise ValueError("unterminated method")


def run(source, directory, name, header):
    callback = method(source, "void BluezEndpoint::OnDevicePropertyChanged(")
    admission = method(source, "void BluezEndpoint::HandleNewDevice(")
    admission = admission[:admission.index("    CHIP_ERROR err;")] + "    ++admitted;\n}\n"
    harness = r'''
#include <gio/gio.h>
#include <cstring>
#include <cstdio>
#include "SCOPE_HEADER"
struct BluezDevice1 { bool connected = true; bool resolved = true; };
bool bluez_device1_get_connected(BluezDevice1 * d) { return d->connected; }
bool bluez_device1_get_services_resolved(BluezDevice1 * d) { return d->resolved; }
#define VerifyOrReturn(condition) do { if (!(condition)) return; } while (false)
extern "C" bool rhythm_chipd_ble_commissioning_active() {
    return rhythm::matter::BluezCommissioningScope::IsActive();
}
class BluezEndpoint {
public:
    bool mIsCentral = true;
    bool existing = false;
    int updates = 0;
    int admitted = 0;
    void HandleNewDevice(BluezDevice1 &);
    void OnDevicePropertyChanged(BluezDevice1 &, GVariant *, const char * const *);
    void UpdateConnectionTable(BluezDevice1 & d) { ++updates; if (!existing) HandleNewDevice(d); }
};
CALLBACK
ADMISSION
GVariant * changed(const char * property) {
    GVariantBuilder builder;
    g_variant_builder_init(&builder, G_VARIANT_TYPE("a{sv}"));
    g_variant_builder_add(&builder, "{sv}", property, g_variant_new_boolean(true));
    return g_variant_ref_sink(g_variant_builder_end(&builder));
}
#define CHECK(value) do { if (!(value)) { std::fprintf(stderr, "check failed: %s\n", #value); return 1; } } while (false)
int main() {
    BluezEndpoint endpoint;
    BluezDevice1 device;
    auto packet = changed("ManufacturerData");
    for (int i = 0; i < 10000; ++i) endpoint.OnDevicePropertyChanged(device, packet, nullptr);
    g_variant_unref(packet);
    CHECK(endpoint.updates == 0);
    packet = changed("RSSI");
    endpoint.OnDevicePropertyChanged(device, packet, nullptr);
    g_variant_unref(packet);
    CHECK(endpoint.updates == 0);
    packet = changed("ServicesResolved");
    endpoint.OnDevicePropertyChanged(device, packet, nullptr);
    CHECK(endpoint.admitted == 0);
    {
        rhythm::matter::BluezCommissioningScope scope;
        endpoint.OnDevicePropertyChanged(device, packet, nullptr);
        CHECK(endpoint.admitted == 1);
    }
    g_variant_unref(packet);
    endpoint.existing = true;
    device.connected = false;
    const int previous = endpoint.updates;
    packet = changed("Connected");
    endpoint.OnDevicePropertyChanged(device, packet, nullptr);
    g_variant_unref(packet);
    CHECK(endpoint.updates == previous + 1);
    const char * invalidated[] = { "Connected", nullptr };
    packet = changed("RSSI");
    endpoint.OnDevicePropertyChanged(device, packet, invalidated);
    g_variant_unref(packet);
    CHECK(endpoint.updates == previous + 2);
    endpoint.mIsCentral = false;
    device.connected = true;
    endpoint.HandleNewDevice(device);
    CHECK(endpoint.admitted == 2);
}
'''.replace("SCOPE_HEADER", str(header)).replace("CALLBACK", callback).replace("ADMISSION", admission)
    code = directory / f"{name}.cc"
    binary = directory / name
    code.write_text(harness)
    flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "gio-2.0"], text=True))
    subprocess.run([os.environ.get("CXX", "c++"), "-std=c++17", str(code), *flags, "-o", str(binary)], check=True)
    return subprocess.run([str(binary)], capture_output=True, text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="unpatched SDK src/platform/Linux/bluez/BluezEndpoint.cpp")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[6]
    patch = root / "tools/os/scripts/build/patches/chip-bluez-idle-guard.patch"
    header = Path(__file__).resolve().parents[1] / "bluez_commissioning_scope.h"
    with tempfile.TemporaryDirectory(prefix="rhythm-bluez-guard-") as temp:
        directory = Path(temp)
        source = args.source.read_text()
        sdk_file = directory / "src/platform/Linux/bluez/BluezEndpoint.cpp"
        sdk_file.parent.mkdir(parents=True)
        sdk_file.write_text(source)
        subprocess.run(["git", "apply", str(patch)], cwd=directory, check=True)
        patched = sdk_file.read_text()
        baseline = run(source, directory, "baseline", header)
        if baseline.returncode == 0:
            raise AssertionError("unpatched callback unexpectedly passed regression")
        result = run(patched, directory, "guarded", header)
        if result.returncode:
            raise AssertionError(result.stderr)
        print("BlueZ callback regression: baseline fails; patched callback passes GLib/admission/cleanup coverage")


if __name__ == "__main__":
    main()
