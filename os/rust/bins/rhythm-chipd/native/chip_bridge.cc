#include "chip_bridge.h"

#include <controller/CHIPDeviceControllerFactory.h>

const char * rhythm_chip_bridge_link_mode(void)
{
    auto & factory = chip::Controller::DeviceControllerFactory::GetInstance();
    chip::Controller::CommissioningParameters parameters;
    parameters.SetRemoteNodeId(1);
    (void) factory;
#if defined(RHYTHM_CHIP_BRIDGE_NATIVE_LIBCHIP)
    return "connectedhomeip-libchip";
#elif defined(RHYTHM_CHIP_BRIDGE_PYTHON_EXTENSION)
    return "connectedhomeip-python-extension";
#else
    return "connectedhomeip-unknown";
#endif
}
