#include "phone_commissioning_discovery.h"
#include <cassert>
#include <iostream>

using namespace chip;

int main()
{
    Inet::IPAddress peer, other;
    assert(Inet::IPAddress::FromString("fe80::1234", peer));
    assert(Inet::IPAddress::FromString("fe80::5678", other));
    PhoneCommissioningDiscovery discovery;
    Dnssd::CommissionNodeData ethernet;
    ethernet.numIPs = 1;
    ethernet.ipAddress[0] = other;
    ethernet.port = 5540;
    ethernet.interfaceId = Inet::InterfaceId(2);
    Dnssd::CommissionNodeData wifi = ethernet;
    wifi.ipAddress[0] = peer;
    wifi.interfaceId = Inet::InterfaceId(7);
    Inet::InterfaceId selected;

    // Enumeration order and interface names cannot select an unrelated link.
    discovery.Begin(peer, 5540);
    discovery.OnDiscoveredDevice(ethernet);
    discovery.OnDiscoveredDevice(wifi);
    assert(discovery.WaitForInterface(std::chrono::milliseconds(1), selected) == CHIP_NO_ERROR);
    assert(selected == wifi.interfaceId);

    // A new request cannot reuse a previous result, a wrong port, or an
    // unscoped advertisement. Timeout must fail without a guessed interface.
    discovery.Begin(peer, 5541);
    discovery.OnDiscoveredDevice(wifi);
    wifi.port = 5541;
    wifi.interfaceId = Inet::InterfaceId::Null();
    discovery.OnDiscoveredDevice(wifi);
    assert(discovery.WaitForInterface(std::chrono::milliseconds(1), selected) == CHIP_ERROR_TIMEOUT);
    assert(!selected.IsPresent());

    // Late discovery after cancellation cannot satisfy the cancelled request.
    discovery.Begin(peer, 5541);
    discovery.End();
    wifi.interfaceId = Inet::InterfaceId(7);
    discovery.OnDiscoveredDevice(wifi);
    assert(discovery.WaitForInterface(std::chrono::milliseconds(1), selected) == CHIP_ERROR_TIMEOUT);
    assert(!selected.IsPresent());
    std::cout << "PASS: link-local handoff resolves the observed peer interface and fences stale results\n";
}
