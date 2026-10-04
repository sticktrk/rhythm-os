#include "../bluez_commissioning_scope.h"

#include <cassert>
#include <thread>

using rhythm::matter::BluezCommissioningScope;

static void FailedCommissioning()
{
    BluezCommissioningScope scope;
    assert(BluezCommissioningScope::IsActive());
    throw 1;
}

int main()
{
    assert(!BluezCommissioningScope::IsActive());
    {
        BluezCommissioningScope outer;
        std::thread bluez([] { assert(BluezCommissioningScope::IsActive()); });
        bluez.join();
        {
            BluezCommissioningScope nested;
            assert(BluezCommissioningScope::IsActive());
        }
        assert(BluezCommissioningScope::IsActive());
    }
    assert(!BluezCommissioningScope::IsActive());
    try { FailedCommissioning(); } catch (int) {}
    assert(!BluezCommissioningScope::IsActive());
    {
        BluezCommissioningScope retry;
        assert(BluezCommissioningScope::IsActive());
    }
    assert(!BluezCommissioningScope::IsActive());
}
