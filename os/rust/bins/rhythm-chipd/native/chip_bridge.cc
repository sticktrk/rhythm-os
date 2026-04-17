#include "chip_bridge.h"

extern "C" void * pychip_DeviceController_StackInit(void);

const char * rhythm_chip_bridge_link_mode(void)
{
    auto * linked_symbol = &pychip_DeviceController_StackInit;
    (void) linked_symbol;
    return "connectedhomeip-linked";
}
