#pragma once

#include <controller/DevicePairingDelegate.h>
#include <lib/support/logging/CHIPLogging.h>
#include <chrono>
#include <condition_variable>
#include <functional>
#include <mutex>
#include <string>
#include <utility>

namespace rhythm::matter {
using namespace chip;
using Controller::DevicePairingDelegate;

class BlockingPairingDelegate final : public DevicePairingDelegate
{
public:
    using CompletionStatusReader = std::function<Controller::CompletionStatus()>;

    explicit BlockingPairingDelegate(CompletionStatusReader readStatus) : mReadStatus(std::move(readStatus)) {}

    void Begin(NodeId nodeId)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        mActive         = true;
        mDone           = false;
        mExpectedNodeId = nodeId;
        mStatus         = CHIP_NO_ERROR;
        mFailedStage.ClearValue();
        mFailureContext.clear();
    }

    void Cancel(CHIP_ERROR error)
    {
        Complete(mExpectedNodeId, error);
    }

    CHIP_ERROR WaitForCompletion(std::chrono::milliseconds timeout, std::string & failureContext)
    {
        std::unique_lock<std::mutex> lock(mMutex);
        failureContext.clear();
        if (!mCondition.wait_for(lock, timeout, [this] { return mDone; }))
        {
            mActive = false;
            return CHIP_ERROR_TIMEOUT;
        }

        mActive = false;
        failureContext = mFailureContext;
        return mStatus;
    }

    void OnPairingComplete(CHIP_ERROR error) override
    {
        if (error != CHIP_NO_ERROR)
        {
            Complete(mExpectedNodeId, error);
        }
    }

    void OnCommissioningStatusUpdate(PeerId peerId, Controller::CommissioningStage stage, CHIP_ERROR error) override
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (Accepts(peerId.GetNodeId()) && stage != Controller::CommissioningStage::kCleanup)
        {
            if (error != CHIP_NO_ERROR)
            {
                mFailedStage.SetValue(stage);
            }
            else
            {
                mFailedStage.ClearValue();
            }
        }
    }

    void OnCommissioningComplete(NodeId nodeId, CHIP_ERROR error) override
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (!Accepts(nodeId))
        {
            return;
        }
        // CHIP calls this before OnCommissioningFailure. Copy the SDK-owned
        // completion metadata on the Matter thread BEFORE waking the RPC
        // waiter. Reading it afterwards races cleanup or the next request.
        if (error != CHIP_NO_ERROR && mFailedStage.HasValue())
        {
            mFailureContext = std::string("commissioning_stage=") + Controller::StageToString(mFailedStage.Value());
            const auto status = mReadStatus();
            if (status.err == error && status.failedStage == mFailedStage && status.networkCommissioningStatus.HasValue())
            {
                mFailureContext += " network_status=" +
                    std::to_string(static_cast<unsigned>(status.networkCommissioningStatus.Value()));
            }
        }
        CompleteLocked(error);
    }

    void OnCommissioningFailure(PeerId peerId, CHIP_ERROR error, Controller::CommissioningStage stageFailed,
                                Optional<Credentials::AttestationVerificationResult> additionalErrorInfo) override
    {
        ChipLogError(Controller,
                     "Matter commissioning failed: node=" ChipLogFormatX64 " stage=%u error=%" CHIP_ERROR_FORMAT
                     " attestation=%d",
                     ChipLogValueX64(peerId.GetNodeId()), static_cast<unsigned>(stageFailed), error.Format(),
                     additionalErrorInfo.HasValue() ? static_cast<int>(additionalErrorInfo.Value()) : -1);
        Complete(peerId.GetNodeId(), error);
    }

private:
    void Complete(NodeId nodeId, CHIP_ERROR error)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (!Accepts(nodeId))
        {
            return;
        }

        CompleteLocked(error);
    }

    bool Accepts(NodeId nodeId) const
    {
        return mActive && !mDone && (mExpectedNodeId == kUndefinedNodeId || nodeId == mExpectedNodeId);
    }

    void CompleteLocked(CHIP_ERROR error)
    {
        mStatus = error;
        mDone   = true;
        mCondition.notify_all();
    }

    CompletionStatusReader mReadStatus;
    Optional<Controller::CommissioningStage> mFailedStage;
    std::string mFailureContext;
    std::mutex mMutex;
    std::condition_variable mCondition;
    bool mActive         = false;
    bool mDone           = false;
    NodeId mExpectedNodeId = kUndefinedNodeId;
    CHIP_ERROR mStatus   = CHIP_NO_ERROR;
};

} // namespace rhythm::matter
