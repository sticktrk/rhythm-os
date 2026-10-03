#!/usr/bin/env python3
"""Keep the shipped HA runtime free of direct-device drivers during transition."""
import subprocess


def main():
    tree = subprocess.check_output(
        ["cargo", "tree", "--locked", "-p", "rhythm-addon", "--target", "all", "--edges", "normal", "--prefix", "none"],
        text=True,
    )
    packages = {line.split()[0] for line in tree.splitlines() if line.strip()}
    forbidden = {"rhythm-matter", "rhythm-chipd", "rhythm-ble", "rhythm-hue",
                 "rhythm-monster", "rhythm-linux-appliance", "bluer", "btleplug"}
    found = sorted(forbidden & packages)
    if found:
        raise SystemExit("Direct-device dependency in HA runtime: " + ", ".join(found))
    if "rhythm-ha" not in packages:
        raise SystemExit("HA runtime is missing its HA adapter")
    print("HA production dependency graph excludes direct protocol and appliance stacks.")


if __name__ == "__main__":
    main()
