#include "../wifi_change_transaction.h"
#include <algorithm>
#include <cassert>
#include <string>
#include <vector>
using rhythm::WifiChangeCode;
struct Device {
    WifiChangeCode preflight = WifiChangeCode::Success, room = WifiChangeCode::Success, add = WifiChangeCode::Success, connect = WifiChangeCode::Success;
    bool admitted = true, arm = true, target = true, complete = true, original = true;
    std::vector<std::string> calls;
    WifiChangeCode Preflight() { calls.push_back("preflight"); return preflight; }
    bool CanArm() { return admitted; }
    bool Arm() { calls.push_back("arm"); return arm; }
    WifiChangeCode MakeRoom() { calls.push_back("room"); return room; }
    WifiChangeCode Add() { calls.push_back("add"); return add; }
    WifiChangeCode Connect() { calls.push_back("connect"); return connect; }
    int targetChecks = 0, targetFailsAfter = -1;
    bool VerifyTarget() { calls.push_back("target"); return target && (targetFailsAfter < 0 || targetChecks++ < targetFailsAfter); }
    bool Complete() { calls.push_back("complete"); return complete; }
    void Rollback() { calls.push_back("rollback"); }
    bool VerifyOriginal() { calls.push_back("original"); return original; }
};
int main() {
    Device success;
    assert(rhythm::ChangeWifi(success).code == WifiChangeCode::Success);
    assert((success.calls == std::vector<std::string>{"preflight","arm","room","add","connect","target","complete","target"}));
    for (auto rejected : {WifiChangeCode::Unsupported, WifiChangeCode::Offline}) {
        Device device; device.preflight = rejected;
        assert(rhythm::ChangeWifi(device).code == rejected);
        assert(device.calls.size() == 1);
    }
    for (auto rejected : {WifiChangeCode::CredentialsRejected, WifiChangeCode::NetworkNotFound, WifiChangeCode::Rejected}) {
        Device device; device.connect = rejected;
        auto result = rhythm::ChangeWifi(device);
        assert(result.code == rejected && result.rollbackVerified);
        assert((device.calls == std::vector<std::string>{"preflight","arm","room","add","connect","rollback","original"}));
    }
    // A Connect response lost because the bulb left the old network is the
    // expected path: verified readback on the target commits the change.
    Device lostConnect; lostConnect.connect = WifiChangeCode::RecoveryRequired;
    assert(rhythm::ChangeWifi(lostConnect).code == WifiChangeCode::Success);
    assert((lostConnect.calls == std::vector<std::string>{"preflight","arm","room","add","connect","target","complete","target"}));
    // The same lost response without target readback rolls back instead.
    Device lostBulb; lostBulb.connect = WifiChangeCode::RecoveryRequired; lostBulb.target = false;
    auto lost = rhythm::ChangeWifi(lostBulb);
    assert(lost.code == WifiChangeCode::RecoveryRequired && lost.rollbackVerified);
    assert((lostBulb.calls == std::vector<std::string>{"preflight","arm","room","add","connect","target","rollback","original"}));
    // A one-slot bulb that refuses staged removal or the new network never
    // reaches Connect, and the fail-safe restores its working network.
    for (int step = 0; step < 2; ++step) {
        Device device; (step == 0 ? device.room : device.add) = WifiChangeCode::NetworkSlots;
        auto refused = rhythm::ChangeWifi(device);
        assert(refused.code == WifiChangeCode::NetworkSlots && refused.rollbackVerified);
        assert(device.calls.back() == "original" && device.calls[device.calls.size() - 2] == "rollback");
        assert(std::find(device.calls.begin(), device.calls.end(), "connect") == device.calls.end());
    }
    Device lostAfterCommit; lostAfterCommit.targetFailsAfter = 1;
    assert(rhythm::ChangeWifi(lostAfterCommit).code == WifiChangeCode::RecoveryRequired);
    Device unverified; unverified.target = false; unverified.original = false;
    auto result = rhythm::ChangeWifi(unverified);
    assert(result.code == WifiChangeCode::VerificationFailed && !result.rollbackVerified);
    Device lostCompletion; lostCompletion.complete = false;
    result = rhythm::ChangeWifi(lostCompletion);
    assert(result.code == WifiChangeCode::RecoveryRequired && !result.rollbackVerified);
    assert(lostCompletion.calls.back() == "complete");
    Device delayed; delayed.admitted = false;
    assert(rhythm::ChangeWifi(delayed).code == WifiChangeCode::RecoveryRequired && delayed.calls.size() == 1);
    Device busy; busy.arm = false;
    assert(rhythm::ChangeWifi(busy).code == WifiChangeCode::FailSafeBusy && busy.calls.size() == 2);
}
