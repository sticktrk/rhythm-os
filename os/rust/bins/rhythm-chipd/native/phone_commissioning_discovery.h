#pragma once

#include <controller/DeviceDiscoveryDelegate.h>
#include <chrono>
#include <condition_variable>
#include <mutex>

// A phone's IPv6 scope identifies its own interface, never the Box's. Resolve
// the handed-off peer through local DNS-SD and retain the interface on which
// that exact address/port was observed. PASE still authenticates the peer.
class PhoneCommissioningDiscovery final : public chip::Controller::DeviceDiscoveryDelegate
{
public:
    void Begin(const chip::Inet::IPAddress & address, uint16_t port)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        mAddress = address;
        mPort = port;
        mInterface = chip::Inet::InterfaceId::Null();
        mActive = true;
    }

    void End()
    {
        std::lock_guard<std::mutex> lock(mMutex);
        mActive = false;
    }

    void OnDiscoveredDevice(const chip::Dnssd::CommissionNodeData & node) override
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (!mActive || mInterface.IsPresent() || node.port != mPort || !node.interfaceId.IsPresent())
        {
            return;
        }
        for (size_t i = 0; i < node.numIPs; ++i)
        {
            if (node.ipAddress[i] == mAddress)
            {
                mInterface = node.interfaceId;
                mCondition.notify_all();
                return;
            }
        }
    }

    CHIP_ERROR WaitForInterface(std::chrono::milliseconds timeout, chip::Inet::InterfaceId & interfaceId)
    {
        std::unique_lock<std::mutex> lock(mMutex);
        const bool found = mCondition.wait_for(lock, timeout, [this] { return mInterface.IsPresent(); });
        mActive = false;
        interfaceId = mInterface;
        return found ? CHIP_NO_ERROR : CHIP_ERROR_TIMEOUT;
    }

private:
    std::mutex mMutex;
    std::condition_variable mCondition;
    chip::Inet::IPAddress mAddress;
    uint16_t mPort = 0;
    chip::Inet::InterfaceId mInterface;
    bool mActive = false;
};
