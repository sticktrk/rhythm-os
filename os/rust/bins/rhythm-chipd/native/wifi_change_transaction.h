#pragma once

// Protocol-independent transaction policy, shared by the native adapter and
// deterministic tests. No request or response text is ever logged here.
namespace rhythm {
enum class WifiChangeCode : unsigned char {
    Success = 0, Unsupported = 1, Offline = 2, NetworkSlots = 3,
    CredentialsRejected = 4, NetworkNotFound = 5, Rejected = 6,
    FailSafeBusy = 7, VerificationFailed = 8, RecoveryRequired = 9,
};
struct WifiChangeResult { WifiChangeCode code; bool rollbackVerified = false; };

template <class Device>
WifiChangeResult ChangeWifi(Device & device)
{
    auto code = device.Preflight();
    if (code != WifiChangeCode::Success) return {code, false};
    // The durable receipt bounds even work delayed by controller admission.
    if (!device.CanArm()) return {WifiChangeCode::RecoveryRequired, false};
    if (!device.Arm()) return {WifiChangeCode::FailSafeBusy, false};
    // Everything from here is staged under the fail-safe: rollback or expiry
    // restores the previous configuration, including a network MakeRoom staged
    // for removal on a bulb without a spare slot.
    code = device.MakeRoom();
    if (code == WifiChangeCode::Success) code = device.Add();
    if (code == WifiChangeCode::Success) {
        code = device.Connect();
        // A bulb moving networks over its operational session commonly cannot
        // deliver the Connect response. Delivery failure is therefore not
        // evidence of failure: only authenticated readback on the target decides.
        if (code == WifiChangeCode::Success)
            code = device.VerifyTarget() ? WifiChangeCode::Success : WifiChangeCode::VerificationFailed;
        else if (code == WifiChangeCode::RecoveryRequired && device.VerifyTarget())
            code = WifiChangeCode::Success;
    }
    if (code != WifiChangeCode::Success) {
        device.Rollback();
        return {code, device.VerifyOriginal()};
    }
    // A lost completion response may mean credentials were committed. Do not
    // claim rollback or retry; the caller retains a recovery-required receipt.
    if (!device.Complete()) return {WifiChangeCode::RecoveryRequired, false};
    if (!device.VerifyTarget()) return {WifiChangeCode::RecoveryRequired, false};
    return {WifiChangeCode::Success, false};
}
} // namespace rhythm
