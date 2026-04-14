#!/bin/bash

set -eu

BOARD_DIR="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
BOOT_DIR="${BINARIES_DIR}/boot"
TMP_DIR="${BUILD_DIR}/genimage.tmp"
GENIMAGE_CFG="${BINARIES_DIR}/genimage-rpiz.cfg"

rm -rf "${BOOT_DIR}" "${TMP_DIR}"
mkdir -p "${BOOT_DIR}" "${TMP_DIR}"

cp -a "${BINARIES_DIR}/rpi-firmware/." "${BOOT_DIR}/"
cp -f "${BINARIES_DIR}/zImage" "${BOOT_DIR}/kernel.img"
cp -f "${BOARD_DIR}/config.txt" "${BOOT_DIR}/config.txt"
cp -f "${BOARD_DIR}/cmdline.txt" "${BOOT_DIR}/cmdline.txt"

if [ -f "${BINARIES_DIR}/bcm2708-rpi-zero.dtb" ]; then
    cp -f "${BINARIES_DIR}/bcm2708-rpi-zero.dtb" "${BOOT_DIR}/"
fi
if [ -f "${BINARIES_DIR}/bcm2708-rpi-zero-w.dtb" ]; then
    cp -f "${BINARIES_DIR}/bcm2708-rpi-zero-w.dtb" "${BOOT_DIR}/"
fi

FILES=()
for i in "${BOOT_DIR}"/*; do
    [ -e "${i}" ] || continue
    FILES+=( "${i#${BINARIES_DIR}/}" )
done

BOOT_FILES=$(printf '\t\t\t"%s",\n' "${FILES[@]}")
sed "s|#BOOT_FILES#|${BOOT_FILES}|" "${BOARD_DIR}/genimage.cfg" > "${GENIMAGE_CFG}"

"${HOST_DIR}/bin/genimage" \
    --rootpath "${TARGET_DIR}" \
    --tmppath "${TMP_DIR}" \
    --inputpath "${BINARIES_DIR}" \
    --outputpath "${BINARIES_DIR}" \
    --config "${GENIMAGE_CFG}"
