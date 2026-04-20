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

const char * rhythm_chip_bridge_link_mode(void);
bool rhythm_chip_bridge_init(const char * storage_path, const char * fabric_id, bool has_ble_controller,
                             uint16_t ble_controller, uint16_t controller_vendor_id, char * error_message,
                             size_t error_message_size);
bool rhythm_chip_bridge_commission_light(const struct rhythm_chip_bridge_commission_request * request,
                                         struct rhythm_chip_bridge_device * device, char * error_message,
                                         size_t error_message_size);
bool rhythm_chip_bridge_probe_light(uint64_t node_id, struct rhythm_chip_bridge_device * device, char * error_message,
                                    size_t error_message_size);
bool rhythm_chip_bridge_decommission_device(uint64_t node_id, bool force, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_on_off(uint64_t node_id, uint16_t endpoint, bool on, char * error_message,
                                   size_t error_message_size);
bool rhythm_chip_bridge_set_brightness(uint64_t node_id, uint16_t endpoint, uint8_t level, bool has_transition_ms,
                                       uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_color_temperature(uint64_t node_id, uint16_t endpoint, uint16_t kelvin, bool has_transition_ms,
                                              uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_xy(uint64_t node_id, uint16_t endpoint, float x, float y, bool has_transition_ms,
                               uint32_t transition_ms, char * error_message, size_t error_message_size);
bool rhythm_chip_bridge_set_hue_saturation(uint64_t node_id, uint16_t endpoint, uint8_t hue, uint8_t saturation,
                                           bool has_transition_ms, uint32_t transition_ms, char * error_message,
                                           size_t error_message_size);
bool rhythm_chip_bridge_read_on_off(uint64_t node_id, uint16_t endpoint, bool * out_on, char * error_message,
                                    size_t error_message_size);
void rhythm_chip_bridge_shutdown(void);

#ifdef __cplusplus
}
#endif
