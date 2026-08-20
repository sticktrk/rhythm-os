#!/bin/sh
set -eu

TEST_DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"
INIT_SCRIPT="$TEST_DIR/../buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d/S42wifi"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-wifi-ipv6.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

# Source function definitions without selecting an init action.
set -- sourced-for-test
. "$INIT_SCRIPT"

IPV6_CONF_ROOT="$TEST_ROOT/ipv6/conf"
LOGFILE="$TEST_ROOT/wifi.log"
mkdir -p "$IPV6_CONF_ROOT/all" "$IPV6_CONF_ROOT/wlan0"
printf '%s\n' 0 >"$IPV6_CONF_ROOT/all/forwarding"
printf '%s\n' 0 >"$IPV6_CONF_ROOT/wlan0/forwarding"
printf '%s\n' 0 >"$IPV6_CONF_ROOT/wlan0/accept_ra"
printf '%s\n' 0 >"$IPV6_CONF_ROOT/wlan0/accept_ra_rt_info_max_plen"

configure_ipv6_ra_rio wlan0
grep -qx 1 "$IPV6_CONF_ROOT/wlan0/accept_ra"
grep -qx 64 "$IPV6_CONF_ROOT/wlan0/accept_ra_rt_info_max_plen"

# Forwarding appliances need accept_ra=2 or Linux suppresses RA processing.
printf '%s\n' 1 >"$IPV6_CONF_ROOT/all/forwarding"
printf '%s\n' 0 >"$IPV6_CONF_ROOT/wlan0/accept_ra"
configure_ipv6_ra_rio wlan0
grep -qx 2 "$IPV6_CONF_ROOT/wlan0/accept_ra"
grep -q 'RIO prefixes through /64' "$LOGFILE"
