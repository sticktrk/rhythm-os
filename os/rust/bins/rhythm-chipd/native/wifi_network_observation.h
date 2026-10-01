#pragma once
#include <string>
#include <vector>
#include <utility>
namespace rhythm {
// Exactly one connected entry is evidence. Saved entries and ambiguous or
// malformed readback cannot identify the currently connected network.
inline bool ConnectedWifiNetwork(const std::vector<std::pair<std::string, bool>> & networks, std::string & ssid)
{
    ssid.clear();
    size_t connected = 0;
    for (const auto & network : networks) {
        if (!network.second) continue;
        ++connected;
        if (network.first.empty() || network.first.size() > 32) return false;
        ssid = network.first;
    }
    if (connected == 1) return true;
    ssid.clear();
    return false;
}
}
