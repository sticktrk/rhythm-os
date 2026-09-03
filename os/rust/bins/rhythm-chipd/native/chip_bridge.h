#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum rhythm_chip_bridge_rendezvous_mode
{
    RHYTHM_CHIP_BRIDGE_RENDEZVOUS_AUTO = 0,
    RHYTHM_CHIP_BRIDGE_RENDEZVOUS_BLE = 1,
    RHYTHM_CHIP_BRIDGE_RENDEZVOUS_ON_NETWORK = 2,
};

enum rhythm_chip_bridge_color_mode_flags
{
    RHYTHM_CHIP_BRIDGE_COLOR_MODE_HUE_SATURATION = 1 << 0,
    RHYTHM_CHIP_BRIDGE_COLOR_MODE_XY = 1 << 1,
    RHYTHM_CHIP_BRIDGE_COLOR_MODE_COLOR_TEMPERATURE = 1 << 2,
};

enum
{
    RHYTHM_CHIP_BRIDGE_STRING_CAPACITY = 128,
};

struct rhythm_chip_bridge_commission_request
{
    const char * setup_payload;
    uint64_t node_id;
    uint8_t rendezvous_mode;
    const char * wifi_ssid;
    const char * wifi_password;
};

struct rhythm_chip_bridge_device
{
    uint64_t node_id;
    uint16_t vendor_id;
    uint16_t product_id;
    uint16_t light_endpoint;
    uint32_t color_mode_flags;
    uint16_t min_kelvin;
    uint16_t max_kelvin;
    uint8_t has_serial_number;
    uint8_t has_min_kelvin;
    uint8_t has_max_kelvin;
    char vendor_name[RHYTHM_CHIP_BRIDGE_STRING_CAPACITY];
    char product_name[RHYTHM_CHIP_BRIDGE_STRING_CAPACITY];
    char serial_number[RHYTHM_CHIP_BRIDGE_STRING_CAPACITY];
};

enum rhythm_chip_bridge_attribute_value_type
{
    RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_BOOL = 1,
    RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_U8 = 2,
    RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_U16 = 3,
    RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_SUBSCRIPTION_ALIVE = 4,
};

enum rhythm_chip_bridge_level_command
{
    RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_MOVE_TO_LEVEL = 0,
    RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_MOVE_TO_LEVEL_WITH_ON_OFF = 1,
    RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_STEP = 2,
    RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_STEP_WITH_ON_OFF = 3,
};

enum rhythm_chip_bridge_level_step_mode
{
    RHYTHM_CHIP_BRIDGE_LEVEL_STEP_MODE_UP = 0,
    RHYTHM_CHIP_BRIDGE_LEVEL_STEP_MODE_DOWN = 1,
};

struct rhythm_chip_bridge_subscription_target
{
    uint64_t node_id;
    uint16_t endpoint;
};

struct rhythm_chip_bridge_group_member
{
    uint64_t node_id;
    uint16_t endpoint;
};

struct rhythm_chip_bridge_group
{
    uint16_t group_id;
    const char * name;
    const struct rhythm_chip_bridge_group_member * members;
    size_t member_count;
};

struct rhythm_chip_bridge_attribute_report
{
    uint64_t node_id;
    uint16_t endpoint;
    uint32_t cluster_id;
    uint32_t attribute_id;
    uint8_t value_type;
    bool bool_value;
    uint64_t unsigned_value;
    uint64_t received_at_unix_ms;
};

/// Terminal failure class for an established subscription. Values are stable
/// wire constants mirrored by rhythm-chipd's Rust FFI shim; they carry no node,
/// fabric, or address identity.
///
/// ADDRESS_RESOLUTION and CASE_SESSION are reserved, not emitted: both kinds of
/// death reach the bridge as CHIP_ERROR_TIMEOUT through OperationalSessionSetup,
/// so the native mapper cannot distinguish them from any other timeout. They
/// stay in the wire contract because Rust does produce them (from the sidecar
/// log classifier) and because a future SDK-code mapping can fill them in.
enum rhythm_chip_bridge_subscription_failure_class
{
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_OTHER = 0,
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_ADDRESS_RESOLUTION = 1,
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_CASE_SESSION = 2,
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_RESOURCE_BUSY = 3,
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_TIMEOUT = 4,
    RHYTHM_CHIP_BRIDGE_SUB_FAIL_PEER_CLOSED = 5,
};

/// One established On/Off subscription that ended. The native bridge never
/// re-subscribes on its own: it reports the termination once and Rust owns
/// cooldown, backoff, and the next attempt.
struct rhythm_chip_bridge_subscription_termination
{
    uint64_t node_id;
    uint16_t endpoint;
    uint32_t chip_error;
    uint8_t failure_class;
};

const char * rhythm_chip_bridge_link_mode(void);
bool rhythm_chip_bridge_init(const char * storage_path, const char * fabric_id, uint64_t operational_fabric_id,
                             const char * ipk_hex, bool has_ble_controller, uint16_t ble_controller,
                             uint16_t controller_vendor_id, uint64_t * out_compressed_fabric_id,
                             char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_commission_light(const struct rhythm_chip_bridge_commission_request * request,
                                         struct rhythm_chip_bridge_device * device, char * error_message,
                                         size_t error_message_size);
bool rhythm_chip_bridge_probe_light(uint64_t node_id, struct rhythm_chip_bridge_device * device, char * error_message,
                                    size_t error_message_size);
bool rhythm_chip_bridge_decommission_device(uint64_t node_id, bool force, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_on_off(uint64_t node_id, uint16_t endpoint, bool on, char * error_message,
                                   size_t error_message_size);
bool rhythm_chip_bridge_configure_group(const struct rhythm_chip_bridge_group * group, char * error_message,
                                        size_t error_message_size);
bool rhythm_chip_bridge_remove_group(uint16_t group_id, const struct rhythm_chip_bridge_group_member * members,
                                     size_t member_count, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_group_on_off(uint16_t group_id, bool on, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_identify_group(uint16_t group_id, uint16_t duration_secs, char * error_message,
                                       size_t error_message_size);
bool rhythm_chip_bridge_set_group_brightness(uint16_t group_id, uint8_t level, bool has_transition_ms,
                                             uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_group_color_temperature(uint16_t group_id, uint16_t kelvin, bool has_transition_ms,
                                                    uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_group_xy(uint16_t group_id, float x, float y, bool has_transition_ms, uint32_t transition_ms,
                                     char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_group_hue_saturation(uint16_t group_id, uint8_t hue, uint8_t saturation,
                                                 bool has_transition_ms, uint32_t transition_ms, char * error_message,
                                                 size_t error_message_size);
bool rhythm_chip_bridge_identify_light(uint64_t node_id, uint16_t endpoint, uint16_t duration_secs, char * error_message,
                                       size_t error_message_size);
bool rhythm_chip_bridge_set_brightness(uint64_t node_id, uint16_t endpoint, uint8_t level, bool has_transition_ms,
                                       uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_run_level_command(uint64_t node_id, uint16_t endpoint, uint8_t command, uint8_t level_or_step,
                                          uint8_t step_mode, bool has_transition_ms, uint32_t transition_ms,
                                          char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_color_temperature(uint64_t node_id, uint16_t endpoint, uint16_t kelvin, bool has_transition_ms,
                                              uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_xy(uint64_t node_id, uint16_t endpoint, float x, float y, bool has_transition_ms,
                               uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_hue_saturation(uint64_t node_id, uint16_t endpoint, uint8_t hue, uint8_t saturation,
                                           bool has_transition_ms, uint32_t transition_ms, char * error_message,
                                           size_t error_message_size);
bool rhythm_chip_bridge_read_on_off(uint64_t node_id, uint16_t endpoint, bool * out_on, char * error_message,
                                    size_t error_message_size);
bool rhythm_chip_bridge_read_light_capability_snapshot(uint64_t node_id, uint16_t endpoint, char * out_json,
                                                       size_t json_size, size_t * out_json_len, char * error_message,
                                                       size_t error_message_size);
bool rhythm_chip_bridge_read_light_state(uint64_t node_id, uint16_t endpoint, char * out_json, size_t json_size,
                                         size_t * out_json_len, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_subscribe_on_off(const struct rhythm_chip_bridge_subscription_target * targets, size_t target_count,
                                         uint16_t min_interval_secs, uint16_t max_interval_secs, bool replace_existing,
                                         char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_drain_attribute_reports(struct rhythm_chip_bridge_attribute_report * reports, size_t reports_capacity,
                                                size_t * out_report_count, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_drain_subscription_terminations(struct rhythm_chip_bridge_subscription_termination * terminations,
                                                        size_t terminations_capacity, size_t * out_termination_count,
                                                        char * error_message, size_t error_message_size);
void rhythm_chip_bridge_shutdown(void);

#ifdef __cplusplus
}
#endif
