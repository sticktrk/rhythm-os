#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

const char * rhythm_chip_bridge_link_mode(void);
bool rhythm_chip_bridge_init(const char * storage_path, const char * fabric_id, bool has_ble_controller,
                             uint16_t ble_controller, char * error_message, size_t error_message_size);
void rhythm_chip_bridge_shutdown(void);

#ifdef __cplusplus
}
#endif
