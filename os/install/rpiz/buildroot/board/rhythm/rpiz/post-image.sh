#!/bin/bash

set -eu

BOARD_DIR="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
TMP_DIR="${BUILD_DIR}/genimage.tmp"
INPUT_DIR="${BUILD_DIR}/genimage.input"
GENIMAGE_CFG="${BINARIES_DIR}/genimage-rpiz.cfg"

rm -rf "${TMP_DIR}" "${INPUT_DIR}"
mkdir -p "${TMP_DIR}" "${INPUT_DIR}"

cp -a "${BINARIES_DIR}/rpi-firmware/." "${INPUT_DIR}/"
cp -f "${BINARIES_DIR}/zImage" "${INPUT_DIR}/kernel.img"
cp -f "${BOARD_DIR}/config.txt" "${INPUT_DIR}/config.txt"
cp -f "${BOARD_DIR}/cmdline.txt" "${INPUT_DIR}/cmdline.txt"
ln -f "${BINARIES_DIR}/rootfs.ext2" "${INPUT_DIR}/rootfs.ext2"

if [ -f "${BINARIES_DIR}/bcm2708-rpi-zero.dtb" ]; then
    cp -f "${BINARIES_DIR}/bcm2708-rpi-zero.dtb" "${INPUT_DIR}/"
fi
if [ -f "${BINARIES_DIR}/bcm2708-rpi-zero-w.dtb" ]; then
    cp -f "${BINARIES_DIR}/bcm2708-rpi-zero-w.dtb" "${INPUT_DIR}/"
fi

FILES=()
for i in "${INPUT_DIR}"/*; do
    [ -e "${i}" ] || continue
    [ "$(basename "${i}")" = "rootfs.ext2" ] && continue
    FILES+=( "${i#${INPUT_DIR}/}" )
done

BOOT_FILES=$(printf '\t\t\t"%s",\n' "${FILES[@]}")
{
    while IFS= read -r line; do
        if [ "$line" = "#BOOT_FILES#" ]; then
            printf '%s' "${BOOT_FILES}"
        else
            printf '%s\n' "$line"
        fi
    done < "${BOARD_DIR}/genimage.cfg"
} > "${GENIMAGE_CFG}"

"${HOST_DIR}/bin/genimage" \
    --rootpath "${TARGET_DIR}" \
    --tmppath "${TMP_DIR}" \
    --inputpath "${INPUT_DIR}" \
    --outputpath "${BINARIES_DIR}" \
    --config "${GENIMAGE_CFG}"
