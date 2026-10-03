#pragma once

#include <atomic>

namespace rhythm::matter {

// The CHIP BlueZ callback runs on its GLib thread. Only commissioning admits
// new central-role connections; an existing connection may always clean up.
class BluezCommissioningScope
{
public:
    BluezCommissioningScope() { sActive.fetch_add(1, std::memory_order_acq_rel); }
    ~BluezCommissioningScope() { sActive.fetch_sub(1, std::memory_order_acq_rel); }
    BluezCommissioningScope(const BluezCommissioningScope &) = delete;
    BluezCommissioningScope & operator=(const BluezCommissioningScope &) = delete;

    static bool IsActive() { return sActive.load(std::memory_order_acquire) != 0; }

private:
    inline static std::atomic<unsigned> sActive{ 0 };
};

} // namespace rhythm::matter
