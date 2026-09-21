#include "../wifi_change_transaction.h"
#include <cassert>
#include <string>
#include <vector>
using rhythm::WifiChangeCode;
struct Device {
    WifiChangeCode preflight = WifiChangeCode::Success, add = WifiChangeCode::Success, connect = WifiChangeCode::Success;
    bool admitted = true, arm = true, target = true, complete = true, original = true;
    std::vector<std::string> calls;
    WifiChangeCode Preflight() { calls.push_back("preflight"); return preflight; }
    bool CanArm() { return admitted; }
    bool Arm() { calls.push_back("arm"); return arm; }
    WifiChangeCode Add() { calls.push_back("add"); return add; }
    WifiChangeCode Connect() { calls.push_back("connect"); return connect; }
    bool VerifyTarget() { calls.push_back("target"); return target; }
    bool Complete() { calls.push_back("complete"); return complete; }
    void Rollback() { calls.push_back("rollback"); }
    bool VerifyOriginal() { calls.push_back("original"); return original; }
};
int main() {
    Device success;
    assert(rhythm::ChangeWifi(success).code == WifiChangeCode::Success);
    assert((success.calls == std::vector<std::string>{"preflight","arm","add","connect","target","complete","target"}));
    for (auto rejected : {WifiChangeCode::Unsupported, WifiChangeCode::Offline, WifiChangeCode::NetworkSlots}) {
        Device device; device.preflight = rejected;
        assert(rhythm::ChangeWifi(device).code == rejected);
        assert(device.calls.size() == 1);
    }
    for (auto rejected : {WifiChangeCode::CredentialsRejected, WifiChangeCode::NetworkNotFound, WifiChangeCode::Rejected, WifiChangeCode::RecoveryRequired}) {
        Device device; device.connect = rejected;
        auto result = rhythm::ChangeWifi(device);
        assert(result.code == rejected && result.rollbackVerified);
        assert((device.calls == std::vector<std::string>{"preflight","arm","add","connect","rollback","original"}));
    }
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
