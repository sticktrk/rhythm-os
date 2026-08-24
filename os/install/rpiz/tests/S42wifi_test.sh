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

# Regressions for issues #515 and #557: the per-interface IPv6 sysctls may not
# exist until wlan0 is up and wpa_supplicant has initialized it. The startup
# path must enable Thread Route Information Option acceptance afterwards.
START_ORDER="$TEST_ROOT/start-order"
INTERFACE_UP="$TEST_ROOT/interface-up"
WPA_READY="$TEST_ROOT/wpa-ready"
has_wifi_config() { return 1; }
modprobe_logged() { :; }
find_wifi_iface() { printf '%s\n' wlan0; }
pid_is_running() { return 1; }
ifconfig() {
    if [ "$1" = wlan0 ] && [ "$2" = up ]; then
        : >"$INTERFACE_UP"
        printf '%s\n' interface-up >>"$START_ORDER"
    fi
}
configure_ipv6_ra_rio() {
    [ -f "$INTERFACE_UP" ]
    [ -f "$WPA_READY" ]
    printf '%s\n' configure-ipv6 >>"$START_ORDER"
}
disable_wifi_power_save() { :; }
start_wpa_supplicant() {
    : >"$WPA_READY"
    printf '%s\n' wpa-started >>"$START_ORDER"
}

start
test "$(sed -n '1p' "$START_ORDER")" = interface-up
test "$(sed -n '2p' "$START_ORDER")" = wpa-started
test "$(sed -n '3p' "$START_ORDER")" = configure-ipv6
