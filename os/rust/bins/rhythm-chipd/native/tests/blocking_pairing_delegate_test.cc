#include "blocking_pairing_delegate.h"
#include <cassert>
#include <iostream>

using namespace chip;
using namespace chip::Controller;
using NetworkStatus = app::Clusters::NetworkCommissioning::NetworkCommissioningStatusEnum;

int main()
{
    CompletionStatus status;
    unsigned reads = 0;
    rhythm::matter::BlockingPairingDelegate delegate([&] { ++reads; return status; });
    const PeerId peer(1, 42);
    std::string context;
    const auto timeout = std::chrono::milliseconds(1);

    // Production SDK ordering: stage failure, successful cleanup, completion,
    // then failure callback. The waiter must already have the detailed cause
    // when the first terminal callback returns.
    status.err = CHIP_ERROR_INTERNAL;
    status.failedStage.SetValue(CommissioningStage::kWiFiNetworkEnable);
    status.networkCommissioningStatus.SetValue(NetworkStatus::kNetworkNotFound);
    delegate.Begin(42);
    delegate.OnPairingComplete(CHIP_NO_ERROR);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkEnable, CHIP_ERROR_INTERNAL);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kCleanup, CHIP_NO_ERROR);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_INTERNAL);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_INTERNAL);
    assert(context == "commissioning_stage=WiFiNetworkEnable network_status=5");
    delegate.OnCommissioningFailure(peer, CHIP_ERROR_INTERNAL, CommissioningStage::kWiFiNetworkEnable, NullOptional);
    assert(reads == 1);

    // New attempt, stale SDK completion metadata: a PASE failure must not
    // inherit the old Wi-Fi diagnosis, and other nodes cannot complete it.
    delegate.Begin(43);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkEnable, CHIP_ERROR_INTERNAL);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_INTERNAL);
    delegate.OnPairingComplete(CHIP_ERROR_TIMEOUT);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_TIMEOUT);
    assert(context.empty());
    assert(reads == 1);

    // A transport timeout in Wi-Fi setup identifies the stage, but must not
    // borrow a previous device response whose error or stage does not match.
    delegate.Begin(42);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkSetup, CHIP_ERROR_TIMEOUT);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_TIMEOUT);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_TIMEOUT);
    assert(context == "commissioning_stage=WiFiNetworkSetup");

    // A stage recovered inside the same attempt is no longer failure evidence.
    delegate.Begin(42);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkEnable, CHIP_ERROR_INTERNAL);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkEnable, CHIP_NO_ERROR);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_TIMEOUT);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_TIMEOUT);
    assert(context.empty());

    // Failure followed by successful retry cannot leak old failure metadata.
    delegate.Begin(42);
    delegate.OnCommissioningComplete(42, CHIP_NO_ERROR);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_NO_ERROR);
    assert(context.empty());

    // Cancellation and timeout fence late callbacks and release the waiter.
    delegate.Begin(42);
    delegate.OnCommissioningStatusUpdate(peer, CommissioningStage::kWiFiNetworkEnable, CHIP_ERROR_INTERNAL);
    delegate.Cancel(CHIP_ERROR_CANCELLED);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_INTERNAL);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_CANCELLED);
    assert(context.empty());
    delegate.Begin(42);
    assert(delegate.WaitForCompletion(timeout, context) == CHIP_ERROR_TIMEOUT);
    delegate.OnCommissioningComplete(42, CHIP_ERROR_INTERNAL);
    assert(context.empty());

    std::cout << "PASS: commissioning details precede terminal handoff and stay request-scoped\n";
}
