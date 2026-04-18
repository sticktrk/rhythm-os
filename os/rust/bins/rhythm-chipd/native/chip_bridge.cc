#include "chip_bridge.h"

#include <app-common/zap-generated/cluster-objects.h>
#include <app/InteractionModelEngine.h>
#include <controller/CHIPCluster.h>
#include <controller/CHIPDeviceController.h>
#include <controller/CHIPDeviceControllerFactory.h>
#include <controller/ExampleOperationalCredentialsIssuer.h>
#include <controller/ExamplePersistentStorage.h>
#include <credentials/GroupDataProviderImpl.h>
#include <credentials/PersistentStorageOpCertStore.h>
#include <credentials/attestation_verifier/DefaultDeviceAttestationVerifier.h>
#include <credentials/attestation_verifier/DeviceAttestationVerifier.h>
#include <crypto/RawKeySessionKeystore.h>
#include <data-model-providers/codegen/Instance.h>
#include <lib/core/CHIPCallback.h>
#include <lib/core/ErrorStr.h>
#include <lib/support/CodeUtils.h>
#include <lib/support/ScopedMemoryBuffer.h>
#include <lib/support/TestGroupData.h>
#include <platform/CHIPDeviceLayer.h>
#include <platform/TestOnlyCommissionableDataProvider.h>
#include <protocols/secure_channel/RendezvousParameters.h>
#include <setup_payload/ManualSetupPayloadParser.h>
#include <setup_payload/QRCodeSetupPayloadParser.h>

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstring>
#include <filesystem>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

using chip::Controller::DeviceCommissioner;
using chip::Controller::DeviceControllerFactory;
using chip::Controller::ExampleOperationalCredentialsIssuer;
using chip::Controller::FactoryInitParams;
using chip::Controller::SetupParams;

namespace {

using namespace chip;
using namespace chip::app::Clusters;
using chip::Controller::ClusterBase;
using chip::Controller::CommissioningParameters;
using chip::Controller::DevicePairingDelegate;
using chip::Controller::DiscoveryType;
using chip::Controller::WiFiCredentials;

constexpr std::chrono::seconds kOperationTimeout(30);
constexpr std::chrono::seconds kCommissioningTimeout(180);
constexpr EndpointId kRootEndpoint = kRootEndpointId;
constexpr VendorId kDefaultControllerVendorId = VendorId::TestVendor1;

// DEV-ONLY attestation verifier that waves every device through.
// Use only on a trusted LAN during rpiz bring-up; production must use
// GetDefaultDACVerifier with a real PAA trust store.
class BypassAttestationVerifier : public chip::Credentials::DeviceAttestationVerifier
{
public:
    void VerifyAttestationInformation(const AttestationInfo & info,
                                      chip::Callback::Callback<OnAttestationInformationVerification> * onCompletion) override
    {
        if (onCompletion != nullptr && onCompletion->mCall != nullptr)
        {
            onCompletion->mCall(onCompletion->mContext, info, chip::Credentials::AttestationVerificationResult::kSuccess);
        }
    }

    chip::Credentials::AttestationVerificationResult
    ValidateCertificationDeclarationSignature(const chip::ByteSpan & cmsEnvelopeBuffer, chip::ByteSpan & certDeclBuffer) override
    {
        certDeclBuffer = cmsEnvelopeBuffer;
        return chip::Credentials::AttestationVerificationResult::kSuccess;
    }

    chip::Credentials::AttestationVerificationResult
    ValidateCertificateDeclarationPayload(const chip::ByteSpan & certDeclBuffer, const chip::ByteSpan & firmwareInfo,
                                          const chip::Credentials::DeviceInfoForAttestation & deviceInfo) override
    {
        (void) certDeclBuffer;
        (void) firmwareInfo;
        (void) deviceInfo;
        return chip::Credentials::AttestationVerificationResult::kSuccess;
    }

    CHIP_ERROR VerifyNodeOperationalCSRInformation(const chip::ByteSpan & nocsrElementsBuffer,
                                                   const chip::ByteSpan & attestationChallengeBuffer,
                                                   const chip::ByteSpan & attestationSignatureBuffer,
                                                   const chip::Crypto::P256PublicKey & dacPublicKey,
                                                   const chip::ByteSpan & csrNonce) override
    {
        (void) nocsrElementsBuffer;
        (void) attestationChallengeBuffer;
        (void) attestationSignatureBuffer;
        (void) dacPublicKey;
        (void) csrNonce;
        return CHIP_NO_ERROR;
    }

    void CheckForRevokedDACChain(const AttestationInfo & info,
                                 chip::Callback::Callback<OnAttestationInformationVerification> * onCompletion) override
    {
        if (onCompletion != nullptr && onCompletion->mCall != nullptr)
        {
            onCompletion->mCall(onCompletion->mContext, info, chip::Credentials::AttestationVerificationResult::kSuccess);
        }
    }
};

void WriteErrorMessage(char * buffer, size_t bufferSize, const std::string & message)
{
    if (buffer == nullptr || bufferSize == 0)
    {
        return;
    }

    const size_t copySize = std::min(bufferSize - 1, message.size());
    std::memcpy(buffer, message.data(), copySize);
    buffer[copySize] = '\0';
}

template <size_t N>
void CopyStringToFixedBuffer(std::string_view value, char (&buffer)[N])
{
    static_assert(N > 0);
    std::memset(buffer, 0, sizeof(buffer));
    const size_t copySize = std::min(value.size(), N - 1);
    if (copySize > 0)
    {
        std::memcpy(buffer, value.data(), copySize);
    }
    buffer[copySize] = '\0';
}

std::string FormatChipError(CHIP_ERROR error, const std::string & context)
{
    if (error == CHIP_NO_ERROR)
    {
        return context;
    }

    std::string message = context + ": " + chip::ErrorStr(error);

    if (message.find("GATT write characteristic operation failed") != std::string::npos)
    {
        message +=
            " (official Matter BLE commissioning reached the device, but the Darwin/CoreBluetooth GATT write failed; "
            "this matches chip-tool behavior on this host)";
    }

    return message;
}

std::string NormalizeSetupPayload(std::string_view setupPayload)
{
    std::string trimmed(setupPayload);
    trimmed.erase(trimmed.begin(),
                  std::find_if(trimmed.begin(), trimmed.end(), [](unsigned char ch) { return !std::isspace(ch); }));
    trimmed.erase(std::find_if(trimmed.rbegin(), trimmed.rend(), [](unsigned char ch) { return !std::isspace(ch); }).base(),
                  trimmed.end());

    if (trimmed.find(chip::kQRCodePrefix) != std::string::npos)
    {
        return trimmed;
    }

    std::string digitsOnly;
    digitsOnly.reserve(trimmed.size());
    for (char ch : trimmed)
    {
        if (std::isdigit(static_cast<unsigned char>(ch)))
        {
            digitsOnly.push_back(ch);
        }
    }
    return digitsOnly;
}

CHIP_ERROR ParseSetupPayload(const std::string & setupPayload, chip::SetupPayload & payload)
{
    if (setupPayload.find(chip::kQRCodePrefix) != std::string::npos)
    {
        return chip::QRCodeSetupPayloadParser(setupPayload).populatePayload(payload);
    }

    return chip::ManualSetupPayloadParser(setupPayload).populatePayload(payload);
}

bool HasCluster(const std::vector<ClusterId> & clusters, ClusterId clusterId)
{
    return std::find(clusters.begin(), clusters.end(), clusterId) != clusters.end();
}

uint16_t MillisecondsToTenths(uint32_t transitionMs)
{
    return static_cast<uint16_t>(std::min<uint32_t>((transitionMs + 99) / 100, UINT16_MAX));
}

uint16_t KelvinToMireds(uint16_t kelvin)
{
    if (kelvin == 0)
    {
        return 0;
    }

    return static_cast<uint16_t>(std::clamp<uint32_t>(1000000u / kelvin, 1u, UINT16_MAX));
}

uint16_t MiredsToKelvin(uint16_t mireds)
{
    if (mireds == 0)
    {
        return 0;
    }

    return static_cast<uint16_t>(std::clamp<uint32_t>(1000000u / mireds, 1u, UINT16_MAX));
}

uint16_t XyToMatterCoordinate(float value)
{
    const float clamped = std::clamp(value, 0.0f, 1.0f);
    return static_cast<uint16_t>(std::lround(clamped * 65535.0f));
}

struct ScheduledWork
{
    std::function<void()> callback;
    std::mutex mutex;
    std::condition_variable condition;
    bool done = false;
};

void PerformScheduledWork(intptr_t arg)
{
    auto * work = reinterpret_cast<ScheduledWork *>(arg);
    work->callback();

    {
        std::lock_guard<std::mutex> lock(work->mutex);
        work->done = true;
    }
    work->condition.notify_all();
}

CHIP_ERROR ExecuteOnMatterThread(const std::function<void()> & callback)
{
    if (chip::DeviceLayer::PlatformMgr().IsChipStackLockedByCurrentThread())
    {
        callback();
        return CHIP_NO_ERROR;
    }

    ScheduledWork work;
    work.callback = callback;

    ReturnErrorOnFailure(
        chip::DeviceLayer::PlatformMgr().ScheduleWork(PerformScheduledWork, reinterpret_cast<intptr_t>(&work)));

    std::unique_lock<std::mutex> lock(work.mutex);
    work.condition.wait(lock, [&work] { return work.done; });
    return CHIP_NO_ERROR;
}

class BlockingOperationBase
{
public:
    CHIP_ERROR WaitForCompletion(std::chrono::seconds timeout)
    {
        std::unique_lock<std::mutex> lock(mMutex);
        if (!mCondition.wait_for(lock, timeout, [this] { return mDone; }))
        {
            return CHIP_ERROR_TIMEOUT;
        }
        return mStatus;
    }

protected:
    void Finish(CHIP_ERROR status)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (mDone)
        {
            return;
        }

        mStatus = status;
        mDone   = true;
        mCondition.notify_all();
    }

private:
    std::mutex mMutex;
    std::condition_variable mCondition;
    bool mDone       = false;
    CHIP_ERROR mStatus = CHIP_NO_ERROR;
};

class DeviceConnectionOperation : public BlockingOperationBase
{
public:
    explicit DeviceConnectionOperation(NodeId nodeId) :
        mNodeId(nodeId), mOnDeviceConnectedCallback(&OnDeviceConnectedFn, this),
        mOnDeviceConnectionFailureCallback(&OnDeviceConnectionFailureFn, this)
    {}

    CHIP_ERROR Start(DeviceCommissioner & commissioner)
    {
        return commissioner.GetConnectedDevice(mNodeId, &mOnDeviceConnectedCallback, &mOnDeviceConnectionFailureCallback);
    }

protected:
    virtual CHIP_ERROR HandleConnected(Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle) = 0;

private:
    static void OnDeviceConnectedFn(void * context, Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle)
    {
        auto * self = static_cast<DeviceConnectionOperation *>(context);
        VerifyOrReturn(self != nullptr);

        CHIP_ERROR err = self->HandleConnected(exchangeMgr, sessionHandle);
        if (err != CHIP_NO_ERROR)
        {
            self->Finish(err);
        }
    }

    static void OnDeviceConnectionFailureFn(void * context, const ScopedNodeId & peerId, CHIP_ERROR error)
    {
        auto * self = static_cast<DeviceConnectionOperation *>(context);
        VerifyOrReturn(self != nullptr);

        ChipLogError(Controller, "Failed to connect to " ChipLogFormatX64 ": %" CHIP_ERROR_FORMAT,
                     ChipLogValueX64(peerId.GetNodeId()), error.Format());
        self->Finish(error);
    }

    NodeId mNodeId;
    chip::Callback::Callback<OnDeviceConnected> mOnDeviceConnectedCallback;
    chip::Callback::Callback<OnDeviceConnectionFailure> mOnDeviceConnectionFailureCallback;
};

template <typename RequestT>
class BlockingInvokeCommandOperation final : public DeviceConnectionOperation
{
public:
    BlockingInvokeCommandOperation(NodeId nodeId, EndpointId endpoint, const RequestT & request) :
        DeviceConnectionOperation(nodeId), mEndpoint(endpoint), mRequest(request)
    {}

protected:
    CHIP_ERROR HandleConnected(Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle) override
    {
        ClusterBase cluster(exchangeMgr, sessionHandle, mEndpoint);
        return cluster.InvokeCommand(mRequest, this, OnSuccess, OnFailure);
    }

private:
    static void OnSuccess(void * context, const typename RequestT::ResponseType & response)
    {
        (void) response;
        auto * self = static_cast<BlockingInvokeCommandOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->Finish(CHIP_NO_ERROR);
    }

    static void OnFailure(void * context, CHIP_ERROR error)
    {
        auto * self = static_cast<BlockingInvokeCommandOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->Finish(error);
    }

    EndpointId mEndpoint;
    RequestT mRequest;
};

template <typename AttributeInfo>
class BlockingReadAttributeOperation final : public DeviceConnectionOperation
{
public:
    using ArgType = typename AttributeInfo::DecodableArgType;
    using CopyFn  = std::function<void(ArgType)>;

    BlockingReadAttributeOperation(NodeId nodeId, EndpointId endpoint, CopyFn onValue) :
        DeviceConnectionOperation(nodeId), mEndpoint(endpoint), mOnValue(std::move(onValue))
    {}

protected:
    CHIP_ERROR HandleConnected(Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle) override
    {
        ClusterBase cluster(exchangeMgr, sessionHandle, mEndpoint);
        return cluster.template ReadAttribute<AttributeInfo>(this, OnSuccess, OnFailure);
    }

private:
    static void OnSuccess(void * context, ArgType value)
    {
        auto * self = static_cast<BlockingReadAttributeOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->mOnValue(value);
        self->Finish(CHIP_NO_ERROR);
    }

    static void OnFailure(void * context, CHIP_ERROR error)
    {
        auto * self = static_cast<BlockingReadAttributeOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->Finish(error);
    }

    EndpointId mEndpoint;
    CopyFn mOnValue;
};

class BlockingPairingDelegate final : public DevicePairingDelegate
{
public:
    void Begin(NodeId nodeId)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        mActive         = true;
        mDone           = false;
        mExpectedNodeId = nodeId;
        mStatus         = CHIP_NO_ERROR;
    }

    void Cancel(CHIP_ERROR error)
    {
        Complete(mExpectedNodeId, error);
    }

    CHIP_ERROR WaitForCompletion(std::chrono::seconds timeout)
    {
        std::unique_lock<std::mutex> lock(mMutex);
        if (!mCondition.wait_for(lock, timeout, [this] { return mDone; }))
        {
            mActive = false;
            return CHIP_ERROR_TIMEOUT;
        }

        mActive = false;
        return mStatus;
    }

    void OnPairingComplete(CHIP_ERROR error) override
    {
        if (error != CHIP_NO_ERROR)
        {
            Complete(mExpectedNodeId, error);
        }
    }

    void OnCommissioningComplete(NodeId nodeId, CHIP_ERROR error) override
    {
        Complete(nodeId, error);
    }

    void OnCommissioningFailure(PeerId peerId, CHIP_ERROR error, Controller::CommissioningStage stageFailed,
                                Optional<Credentials::AttestationVerificationResult> additionalErrorInfo) override
    {
        (void) stageFailed;
        (void) additionalErrorInfo;
        Complete(peerId.GetNodeId(), error);
    }

private:
    void Complete(NodeId nodeId, CHIP_ERROR error)
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (!mActive || mDone || (mExpectedNodeId != kUndefinedNodeId && nodeId != mExpectedNodeId))
        {
            return;
        }

        mStatus = error;
        mDone   = true;
        mCondition.notify_all();
    }

    std::mutex mMutex;
    std::condition_variable mCondition;
    bool mActive         = false;
    bool mDone           = false;
    NodeId mExpectedNodeId = kUndefinedNodeId;
    CHIP_ERROR mStatus   = CHIP_NO_ERROR;
};

class ChipBridgeContext
{
public:
    CHIP_ERROR Init(const char * storagePath, const char * fabricId, bool hasBleController, uint16_t bleController,
                    uint16_t controllerVendorId)
    {
        std::lock_guard<std::mutex> lock(mMutex);

        const std::string requestedStorage = storagePath != nullptr ? storagePath : "";
        if (requestedStorage.empty())
        {
            return CHIP_ERROR_INVALID_ARGUMENT;
        }

        if (mCommissioner != nullptr)
        {
            if (requestedStorage == mStoragePath)
            {
                VerifyOrReturnError(mControllerVendorId == controllerVendorId, CHIP_ERROR_INCORRECT_STATE);
                return CHIP_NO_ERROR;
            }
            return CHIP_ERROR_INCORRECT_STATE;
        }

        mStoragePath = requestedStorage;
        mFabricId    = fabricId != nullptr ? fabricId : "";
        mControllerVendorId = controllerVendorId;

        if (!mFactoryInitialized)
        {
            ReturnErrorOnFailure(InitializeFactory(hasBleController, bleController));
        }

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err]() { err = InitializeCommissioner(); }));
        return err;
    }

    CHIP_ERROR CommissionLight(const rhythm_chip_bridge_commission_request & request, rhythm_chip_bridge_device & device)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);
        VerifyOrReturnError(request.setup_payload != nullptr, CHIP_ERROR_INVALID_ARGUMENT);

        const std::string setupPayload = NormalizeSetupPayload(request.setup_payload);
        VerifyOrReturnError(!setupPayload.empty(), CHIP_ERROR_INVALID_ARGUMENT);

        CommissioningParameters commissioningParams;
        const std::string wifiSsid     = request.wifi_ssid != nullptr ? request.wifi_ssid : "";
        const std::string wifiPassword = request.wifi_password != nullptr ? request.wifi_password : "";
        if (!wifiSsid.empty() || !wifiPassword.empty())
        {
            const ByteSpan ssidSpan(reinterpret_cast<const uint8_t *>(wifiSsid.data()), wifiSsid.size());
            const ByteSpan passwordSpan(reinterpret_cast<const uint8_t *>(wifiPassword.data()), wifiPassword.size());
            commissioningParams.SetWiFiCredentials(WiFiCredentials(ssidSpan, passwordSpan));
        }

        CHIP_ERROR err = CHIP_NO_ERROR;
        mPairingDelegate.Begin(request.node_id);

        switch (request.rendezvous_mode)
        {
        case RHYTHM_CHIP_BRIDGE_RENDEZVOUS_BLE: {
            SetupPayload parsedPayload;
            ReturnErrorOnFailure(ParseSetupPayload(setupPayload, parsedPayload));
            ReturnErrorOnFailure(StartBlePairing(request.node_id, parsedPayload, commissioningParams, err));
            break;
        }
        case RHYTHM_CHIP_BRIDGE_RENDEZVOUS_ON_NETWORK:
            ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err, &request, &setupPayload, &commissioningParams]() {
                err = mCommissioner->PairDevice(request.node_id, setupPayload.c_str(), commissioningParams,
                                                DiscoveryType::kDiscoveryNetworkOnly);
            }));
            break;
        case RHYTHM_CHIP_BRIDGE_RENDEZVOUS_AUTO:
        default:
            ReturnErrorOnFailure(StartAutoPairing(request.node_id, setupPayload, wifiSsid, wifiPassword, commissioningParams, err));
            break;
        }

        if (err != CHIP_NO_ERROR)
        {
            mPairingDelegate.Cancel(err);
            return err;
        }

        ReturnErrorOnFailure(mPairingDelegate.WaitForCompletion(kCommissioningTimeout));
        return ProbeLight(request.node_id, device);
    }

    CHIP_ERROR StartBlePairing(NodeId nodeId, const SetupPayload & parsedPayload,
                               CommissioningParameters & commissioningParams, CHIP_ERROR & outErr)
    {
#if CONFIG_NETWORK_LAYER_BLE
        RendezvousParameters rendezvousParams;
        rendezvousParams.SetSetupPINCode(parsedPayload.setUpPINCode);
        rendezvousParams.SetBleLayer(chip::DeviceLayer::ConnectivityMgr().GetBleLayer());
        rendezvousParams.SetPeerAddress(Transport::PeerAddress::BLE());

        if (parsedPayload.discriminator.IsShortDiscriminator())
        {
            rendezvousParams.SetSetupDiscriminator(parsedPayload.discriminator);
        }
        else
        {
            rendezvousParams.SetDiscriminator(parsedPayload.discriminator.GetLongValue());
        }

        return ExecuteOnMatterThread([this, &outErr, nodeId, &rendezvousParams, &commissioningParams]() {
            outErr = mCommissioner->PairDevice(nodeId, rendezvousParams, commissioningParams);
        });
#else
        (void) nodeId;
        (void) parsedPayload;
        (void) commissioningParams;
        (void) outErr;
        mPairingDelegate.Cancel(CHIP_ERROR_NOT_IMPLEMENTED);
        return CHIP_ERROR_NOT_IMPLEMENTED;
#endif
    }

    CHIP_ERROR StartAutoPairing(NodeId nodeId, const std::string & setupPayload, const std::string & wifiSsid,
                                const std::string & wifiPassword, CommissioningParameters & commissioningParams,
                                CHIP_ERROR & outErr)
    {
        SetupPayload parsedPayload;
        const bool parsed = (ParseSetupPayload(setupPayload, parsedPayload) == CHIP_NO_ERROR);
        const bool hasWiFiCredentials = !wifiSsid.empty() || !wifiPassword.empty();

        if (parsed)
        {
            const auto rendezvousInformation = parsedPayload.rendezvousInformation;
            if (rendezvousInformation.HasValue())
            {
                const auto flags = rendezvousInformation.Value();
                const bool supportsBle = flags.Has(RendezvousInformationFlag::kBLE);
                const bool supportsOnNetwork = flags.Has(RendezvousInformationFlag::kOnNetwork);

                // Prefer explicit BLE when onboarding a fresh Wi-Fi device or when BLE is the only advertised rendezvous path.
                if (supportsBle && (hasWiFiCredentials || !supportsOnNetwork))
                {
                    return StartBlePairing(nodeId, parsedPayload, commissioningParams, outErr);
                }

                if (supportsOnNetwork && !supportsBle)
                {
                    return ExecuteOnMatterThread([this, &outErr, nodeId, &setupPayload, &commissioningParams]() {
                        outErr = mCommissioner->PairDevice(nodeId, setupPayload.c_str(), commissioningParams,
                                                           DiscoveryType::kDiscoveryNetworkOnly);
                    });
                }
            }

            if (hasWiFiCredentials)
            {
                return StartBlePairing(nodeId, parsedPayload, commissioningParams, outErr);
            }
        }

        return ExecuteOnMatterThread([this, &outErr, nodeId, &setupPayload, &commissioningParams]() {
            outErr = mCommissioner->PairDevice(nodeId, setupPayload.c_str(), commissioningParams, DiscoveryType::kAll);
        });
    }

    CHIP_ERROR ProbeLight(NodeId nodeId, rhythm_chip_bridge_device & device)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        std::memset(&device, 0, sizeof(device));
        device.node_id = nodeId;

        std::string vendorName;
        std::string productName;
        std::string serialNumber;
        VendorId vendorId = VendorId::Common;
        uint16_t productId = 0;

        ReturnErrorOnFailure(ReadCharSpanAttribute<BasicInformation::Attributes::VendorName::TypeInfo>(nodeId, kRootEndpoint, vendorName));
        ReturnErrorOnFailure(
            ReadCharSpanAttribute<BasicInformation::Attributes::ProductName::TypeInfo>(nodeId, kRootEndpoint, productName));
        ReturnErrorOnFailure(ReadValueAttribute<BasicInformation::Attributes::VendorID::TypeInfo>(nodeId, kRootEndpoint, vendorId));
        ReturnErrorOnFailure(ReadValueAttribute<BasicInformation::Attributes::ProductID::TypeInfo>(nodeId, kRootEndpoint, productId));

        if (ReadCharSpanAttribute<BasicInformation::Attributes::SerialNumber::TypeInfo>(nodeId, kRootEndpoint, serialNumber) ==
            CHIP_NO_ERROR)
        {
            device.has_serial_number = true;
            CopyStringToFixedBuffer(serialNumber, device.serial_number);
        }

        std::vector<EndpointId> candidateEndpoints;
        if (ReadEndpointListAttribute<Descriptor::Attributes::PartsList::TypeInfo>(nodeId, kRootEndpoint, candidateEndpoints) !=
                CHIP_NO_ERROR ||
            candidateEndpoints.empty())
        {
            candidateEndpoints = { 1 };
        }

        EndpointId lightEndpoint = kInvalidEndpointId;
        std::vector<ClusterId> serverList;
        for (EndpointId endpoint : candidateEndpoints)
        {
            std::vector<ClusterId> clusters;
            if (ReadClusterListAttribute<Descriptor::Attributes::ServerList::TypeInfo>(nodeId, endpoint, clusters) != CHIP_NO_ERROR)
            {
                continue;
            }

            if (HasCluster(clusters, OnOff::Id))
            {
                lightEndpoint = endpoint;
                serverList    = std::move(clusters);
                break;
            }
        }

        VerifyOrReturnError(lightEndpoint != kInvalidEndpointId, CHIP_ERROR_NOT_FOUND);

        device.vendor_id     = static_cast<uint16_t>(vendorId);
        device.product_id    = productId;
        device.light_endpoint = lightEndpoint;
        CopyStringToFixedBuffer(vendorName, device.vendor_name);
        CopyStringToFixedBuffer(productName, device.product_name);

        if (HasCluster(serverList, ColorControl::Id))
        {
            chip::BitMask<ColorControl::ColorCapabilitiesBitmap> colorCapabilities;
            if (ReadValueAttribute<ColorControl::Attributes::ColorCapabilities::TypeInfo>(nodeId, lightEndpoint, colorCapabilities) ==
                CHIP_NO_ERROR)
            {
                if (colorCapabilities.Has(ColorControl::ColorCapabilitiesBitmap::kHueSaturation))
                {
                    device.color_mode_flags |= RHYTHM_CHIP_BRIDGE_COLOR_MODE_HUE_SATURATION;
                }
                if (colorCapabilities.Has(ColorControl::ColorCapabilitiesBitmap::kXy))
                {
                    device.color_mode_flags |= RHYTHM_CHIP_BRIDGE_COLOR_MODE_XY;
                }
                if (colorCapabilities.Has(ColorControl::ColorCapabilitiesBitmap::kColorTemperature))
                {
                    device.color_mode_flags |= RHYTHM_CHIP_BRIDGE_COLOR_MODE_COLOR_TEMPERATURE;

                    uint16_t minMireds = 0;
                    uint16_t maxMireds = 0;
                    if (ReadValueAttribute<ColorControl::Attributes::ColorTempPhysicalMinMireds::TypeInfo>(nodeId, lightEndpoint,
                                                                                                          minMireds) == CHIP_NO_ERROR)
                    {
                        device.max_kelvin     = MiredsToKelvin(minMireds);
                        device.has_max_kelvin = device.max_kelvin != 0;
                    }
                    if (ReadValueAttribute<ColorControl::Attributes::ColorTempPhysicalMaxMireds::TypeInfo>(nodeId, lightEndpoint,
                                                                                                          maxMireds) == CHIP_NO_ERROR)
                    {
                        device.min_kelvin     = MiredsToKelvin(maxMireds);
                        device.has_min_kelvin = device.min_kelvin != 0;
                    }
                }
            }
        }

        return CHIP_NO_ERROR;
    }

    CHIP_ERROR DecommissionDevice(NodeId nodeId, bool force)
    {
        (void) force;
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err, nodeId]() { err = mCommissioner->UnpairDevice(nodeId); }));
        return err;
    }

    CHIP_ERROR SetOnOff(NodeId nodeId, EndpointId endpoint, bool on)
    {
        if (on)
        {
            OnOff::Commands::On::Type request;
            return InvokeCommand(nodeId, endpoint, request);
        }

        OnOff::Commands::Off::Type request;
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR SetBrightness(NodeId nodeId, EndpointId endpoint, uint8_t level, std::optional<uint32_t> transitionMs)
    {
        LevelControl::Commands::MoveToLevelWithOnOff::Type request;
        request.level          = level;
        request.transitionTime = transitionMs.has_value() ? chip::app::DataModel::Nullable<uint16_t>(MillisecondsToTenths(*transitionMs))
                                                          : chip::app::DataModel::Nullable<uint16_t>();
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR SetColorTemperature(NodeId nodeId, EndpointId endpoint, uint16_t kelvin, std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(kelvin > 0, CHIP_ERROR_INVALID_ARGUMENT);

        ColorControl::Commands::MoveToColorTemperature::Type request;
        request.colorTemperatureMireds = KelvinToMireds(kelvin);
        request.transitionTime         = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR SetXy(NodeId nodeId, EndpointId endpoint, float x, float y, std::optional<uint32_t> transitionMs)
    {
        ColorControl::Commands::MoveToColor::Type request;
        request.colorX         = XyToMatterCoordinate(x);
        request.colorY         = XyToMatterCoordinate(y);
        request.transitionTime = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR ReadOnOff(NodeId nodeId, EndpointId endpoint, bool & on)
    {
        return ReadValueAttribute<OnOff::Attributes::OnOff::TypeInfo>(nodeId, endpoint, on);
    }

    void Shutdown()
    {
        std::lock_guard<std::mutex> lock(mMutex);

        if (!mFactoryInitialized)
        {
            return;
        }

        CHIP_ERROR ignored = ExecuteOnMatterThread([this]() {
            if (mCommissioner != nullptr)
            {
                mCommissioner->Shutdown();
                mCommissioner.reset();
            }
        });
        (void) ignored;

        if (mEventLoopStarted)
        {
            chip::DeviceLayer::PlatformMgr().StopEventLoopTask();
            mEventLoopStarted = false;
        }

        DeviceControllerFactory::GetInstance().ReleaseSystemState();
        DeviceControllerFactory::GetInstance().Shutdown();

        mFactoryInitialized = false;
        mStorage.reset();
        mStoragePath.clear();
        mFabricId.clear();
        mControllerVendorId = static_cast<uint16_t>(kDefaultControllerVendorId);
    }

private:
    CHIP_ERROR InitializeFactory(bool hasBleController, uint16_t bleController)
    {
        ReturnErrorOnFailure(chip::Platform::MemoryInit());

#if CHIP_DEVICE_LAYER_TARGET_LINUX && CHIP_DEVICE_CONFIG_ENABLE_CHIPOBLE
        // Always ConfigureBle in central role — we are a commissioner, not a
        // commissionable device. Without this call, mIsCentral stays at its
        // default false, which registers a peripheral GATT service and makes
        // HandleNewConnection skip posting kPlatformLinuxBLECentralConnected,
        // so the BTP endpoint is never established after a successful GATT
        // connect. Fall back to adapter 0 (hci0) when the caller did not
        // select a specific controller.
        const uint16_t adapterId = hasBleController ? bleController : 0;
        ReturnErrorOnFailure(
            chip::DeviceLayer::Internal::BLEMgrImpl().ConfigureBle(adapterId, /* BLE central */ true));
#else
        (void) hasBleController;
        (void) bleController;
#endif

        ReturnErrorOnFailure(chip::DeviceLayer::PlatformMgr().InitChipStack());

        auto storagePath = std::filesystem::path(mStoragePath);
        auto directory   = storagePath.has_parent_path() ? storagePath.parent_path() : std::filesystem::path(".");
        auto name        = storagePath.stem().string();

        mStorage = std::make_unique<PersistentStorage>();
        ReturnErrorOnFailure(mStorage->Init(name.empty() ? nullptr : name.c_str(), directory.string().c_str()));

        FactoryInitParams factoryParams;
        factoryParams.fabricIndependentStorage = mStorage.get();
        factoryParams.sessionKeystore          = &mSessionKeystore;
        factoryParams.dataModelProvider        = chip::app::CodegenDataModelProviderInstance(mStorage.get());

        mGroupDataProvider.SetStorageDelegate(mStorage.get());
        mGroupDataProvider.SetSessionKeystore(factoryParams.sessionKeystore);
        ReturnErrorOnFailure(mGroupDataProvider.Init());
        chip::Credentials::SetGroupDataProvider(&mGroupDataProvider);
        factoryParams.groupDataProvider = &mGroupDataProvider;

        ReturnErrorOnFailure(mOpCertStore.Init(mStorage.get()));
        factoryParams.opCertStore = &mOpCertStore;

        static chip::DeviceLayer::TestOnlyCommissionableDataProvider sCommissionableDataProvider;
        chip::DeviceLayer::SetCommissionableDataProvider(&sCommissionableDataProvider);

        ReturnErrorOnFailure(DeviceControllerFactory::GetInstance().Init(factoryParams));
        DeviceControllerFactory::GetInstance().RetainSystemState();

        ReturnErrorOnFailure(chip::DeviceLayer::PlatformMgr().StartEventLoopTask());
        mEventLoopStarted   = true;
        mFactoryInitialized = true;

#if CHIP_DEVICE_LAYER_TARGET_LINUX && CHIP_DEVICE_CONFIG_ENABLE_CHIPOBLE
        // We're a commissioner, not a commissionable device — defensively
        // disable BLE peripheral advertising in case AUTOSTART flipped it on
        // before ConfigureBle set the central-role flag.
        (void) chip::DeviceLayer::ConnectivityMgr().SetBLEAdvertisingEnabled(false);
#endif
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR InitializeCommissioner()
    {
        if (mCommissioner != nullptr)
        {
            return CHIP_NO_ERROR;
        }

        // DEV-ONLY: skip Matter device attestation so we can commission Matter
        // devices whose PAA isn't in CHIP's test trust store (e.g. Espressif-
        // based bulbs) while we're iterating on the rpiz appliance. Replace
        // with GetDefaultDACVerifier + a real PAA trust store before shipping.
        static BypassAttestationVerifier sBypassVerifier;
        chip::Credentials::SetDeviceAttestationVerifier(&sBypassVerifier);

        auto commissioner = std::make_unique<DeviceCommissioner>();

        SetupParams commissionerParams;
        commissionerParams.deviceAttestationVerifier      = &sBypassVerifier;
        commissionerParams.operationalCredentialsDelegate = &mOperationalCredentialsIssuer;
        commissionerParams.pairingDelegate               = &mPairingDelegate;
        commissionerParams.controllerVendorId            = static_cast<VendorId>(mControllerVendorId);

        ReturnErrorOnFailure(mOperationalKeypair.Initialize(chip::Crypto::ECPKeyTarget::ECDSA));
        commissionerParams.operationalKeypair = &mOperationalKeypair;

        ReturnErrorOnFailure(mOperationalCredentialsIssuer.Initialize(*mStorage));

        chip::Platform::ScopedMemoryBuffer<uint8_t> noc;
        chip::Platform::ScopedMemoryBuffer<uint8_t> icac;
        chip::Platform::ScopedMemoryBuffer<uint8_t> rcac;

        VerifyOrReturnError(noc.Alloc(chip::Controller::kMaxCHIPDERCertLength), CHIP_ERROR_NO_MEMORY);
        VerifyOrReturnError(icac.Alloc(chip::Controller::kMaxCHIPDERCertLength), CHIP_ERROR_NO_MEMORY);
        VerifyOrReturnError(rcac.Alloc(chip::Controller::kMaxCHIPDERCertLength), CHIP_ERROR_NO_MEMORY);

        chip::MutableByteSpan nocSpan(noc.Get(), chip::Controller::kMaxCHIPDERCertLength);
        chip::MutableByteSpan icacSpan(icac.Get(), chip::Controller::kMaxCHIPDERCertLength);
        chip::MutableByteSpan rcacSpan(rcac.Get(), chip::Controller::kMaxCHIPDERCertLength);

        ReturnErrorOnFailure(mOperationalCredentialsIssuer.GenerateNOCChainAfterValidation(
            mStorage->GetLocalNodeId(), /* fabricId = */ 1, mStorage->GetCommissionerCATs(), mOperationalKeypair.Pubkey(), rcacSpan,
            icacSpan, nocSpan));

        commissionerParams.controllerNOC  = nocSpan;
        commissionerParams.controllerICAC = icacSpan;
        commissionerParams.controllerRCAC = rcacSpan;

        ReturnErrorOnFailure(DeviceControllerFactory::GetInstance().SetupCommissioner(commissionerParams, *commissioner));

        uint8_t compressedFabricId[sizeof(uint64_t)] = { 0 };
        chip::MutableByteSpan compressedFabricIdSpan(compressedFabricId);
        ReturnErrorOnFailure(commissioner->GetCompressedFabricIdBytes(compressedFabricIdSpan));

        const chip::ByteSpan defaultIpk = chip::GroupTesting::DefaultIpkValue::GetDefaultIpk();
        ReturnErrorOnFailure(chip::Credentials::SetSingleIpkEpochKey(
            &mGroupDataProvider, commissioner->GetFabricIndex(), defaultIpk, compressedFabricIdSpan));

        mCommissioner = std::move(commissioner);
        return CHIP_NO_ERROR;
    }

    template <typename OperationT>
    CHIP_ERROR RunConnectionOperation(OperationT & operation, std::chrono::seconds timeout = kOperationTimeout)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err, &operation]() { err = operation.Start(*mCommissioner); }));
        ReturnErrorOnFailure(err);
        return operation.WaitForCompletion(timeout);
    }

    template <typename RequestT>
    CHIP_ERROR InvokeCommand(NodeId nodeId, EndpointId endpoint, const RequestT & request)
    {
        BlockingInvokeCommandOperation<RequestT> operation(nodeId, endpoint, request);
        return RunConnectionOperation(operation);
    }

    template <typename AttributeInfo, typename CopyFn>
    CHIP_ERROR ReadAttribute(NodeId nodeId, EndpointId endpoint, CopyFn onValue)
    {
        BlockingReadAttributeOperation<AttributeInfo> operation(nodeId, endpoint, std::move(onValue));
        return RunConnectionOperation(operation);
    }

    template <typename AttributeInfo>
    CHIP_ERROR ReadCharSpanAttribute(NodeId nodeId, EndpointId endpoint, std::string & out)
    {
        return ReadAttribute<AttributeInfo>(nodeId, endpoint, [&out](chip::CharSpan value) {
            out.assign(value.data(), value.size());
        });
    }

    template <typename AttributeInfo, typename ValueT>
    CHIP_ERROR ReadValueAttribute(NodeId nodeId, EndpointId endpoint, ValueT & out)
    {
        return ReadAttribute<AttributeInfo>(nodeId, endpoint, [&out](const auto & value) { out = value; });
    }

    template <typename AttributeInfo>
    CHIP_ERROR ReadEndpointListAttribute(NodeId nodeId, EndpointId endpoint, std::vector<EndpointId> & out)
    {
        CHIP_ERROR iterErr = CHIP_NO_ERROR;
        CHIP_ERROR err     = ReadAttribute<AttributeInfo>(nodeId, endpoint, [&out, &iterErr](const auto & value) {
            out.clear();
            auto iter = value.begin();
            while (iter.Next())
            {
                out.push_back(iter.GetValue());
            }
            iterErr = iter.GetStatus();
        });
        return err == CHIP_NO_ERROR ? iterErr : err;
    }

    template <typename AttributeInfo>
    CHIP_ERROR ReadClusterListAttribute(NodeId nodeId, EndpointId endpoint, std::vector<ClusterId> & out)
    {
        CHIP_ERROR iterErr = CHIP_NO_ERROR;
        CHIP_ERROR err     = ReadAttribute<AttributeInfo>(nodeId, endpoint, [&out, &iterErr](const auto & value) {
            out.clear();
            auto iter = value.begin();
            while (iter.Next())
            {
                out.push_back(iter.GetValue());
            }
            iterErr = iter.GetStatus();
        });
        return err == CHIP_NO_ERROR ? iterErr : err;
    }

    std::mutex mMutex;
    std::unique_ptr<PersistentStorage> mStorage;
    chip::Credentials::GroupDataProviderImpl mGroupDataProvider;
    chip::Credentials::PersistentStorageOpCertStore mOpCertStore;
    ExampleOperationalCredentialsIssuer mOperationalCredentialsIssuer;
    chip::Crypto::RawKeySessionKeystore mSessionKeystore;
    chip::Crypto::P256Keypair mOperationalKeypair;
    BlockingPairingDelegate mPairingDelegate;
    std::unique_ptr<DeviceCommissioner> mCommissioner;
    std::string mStoragePath;
    std::string mFabricId;
    uint16_t mControllerVendorId = static_cast<uint16_t>(kDefaultControllerVendorId);
    bool mFactoryInitialized = false;
    bool mEventLoopStarted   = false;
};

ChipBridgeContext gContext;

bool HandleBridgeResult(CHIP_ERROR err, char * errorMessage, size_t errorMessageSize, const std::string & context)
{
    if (err != CHIP_NO_ERROR)
    {
        WriteErrorMessage(errorMessage, errorMessageSize, FormatChipError(err, context));
        return false;
    }

    WriteErrorMessage(errorMessage, errorMessageSize, "");
    return true;
}

} // namespace

const char * rhythm_chip_bridge_link_mode(void)
{
    auto & factory = chip::Controller::DeviceControllerFactory::GetInstance();
    chip::Controller::CommissioningParameters parameters;
    parameters.SetRemoteNodeId(1);
    (void) factory;
#if defined(RHYTHM_CHIP_BRIDGE_NATIVE_LIBCHIP)
    return "connectedhomeip-libchip";
#elif defined(RHYTHM_CHIP_BRIDGE_PYTHON_EXTENSION)
    return "connectedhomeip-python-extension";
#else
    return "connectedhomeip-unknown";
#endif
}

bool rhythm_chip_bridge_init(const char * storage_path, const char * fabric_id, bool has_ble_controller,
                             uint16_t ble_controller, uint16_t controller_vendor_id, char * error_message,
                             size_t error_message_size)
{
    return HandleBridgeResult(
        gContext.Init(storage_path, fabric_id, has_ble_controller, ble_controller, controller_vendor_id), error_message,
        error_message_size, "initializing CHIP controller bridge");
}

bool rhythm_chip_bridge_commission_light(const struct rhythm_chip_bridge_commission_request * request,
                                         struct rhythm_chip_bridge_device * device, char * error_message,
                                         size_t error_message_size)
{
    if (request == nullptr || device == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "commission_light requires request and device");
        return false;
    }

    return HandleBridgeResult(gContext.CommissionLight(*request, *device), error_message, error_message_size,
                              "commissioning Matter light");
}

bool rhythm_chip_bridge_probe_light(uint64_t node_id, struct rhythm_chip_bridge_device * device, char * error_message,
                                    size_t error_message_size)
{
    if (device == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "probe_light requires device");
        return false;
    }

    return HandleBridgeResult(gContext.ProbeLight(node_id, *device), error_message, error_message_size,
                              "probing Matter light");
}

bool rhythm_chip_bridge_decommission_device(uint64_t node_id, bool force, char * error_message, size_t error_message_size)
{
    return HandleBridgeResult(gContext.DecommissionDevice(node_id, force), error_message, error_message_size,
                              "decommissioning Matter device");
}

bool rhythm_chip_bridge_set_on_off(uint64_t node_id, uint16_t endpoint, bool on, char * error_message,
                                   size_t error_message_size)
{
    return HandleBridgeResult(gContext.SetOnOff(node_id, endpoint, on), error_message, error_message_size,
                              "setting Matter on/off");
}

bool rhythm_chip_bridge_set_brightness(uint64_t node_id, uint16_t endpoint, uint8_t level, bool has_transition_ms,
                                       uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetBrightness(node_id, endpoint, level, transition), error_message, error_message_size,
                              "setting Matter brightness");
}

bool rhythm_chip_bridge_set_color_temperature(uint64_t node_id, uint16_t endpoint, uint16_t kelvin, bool has_transition_ms,
                                              uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetColorTemperature(node_id, endpoint, kelvin, transition), error_message,
                              error_message_size, "setting Matter color temperature");
}

bool rhythm_chip_bridge_set_xy(uint64_t node_id, uint16_t endpoint, float x, float y, bool has_transition_ms,
                               uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetXy(node_id, endpoint, x, y, transition), error_message, error_message_size,
                              "setting Matter xy color");
}

bool rhythm_chip_bridge_read_on_off(uint64_t node_id, uint16_t endpoint, bool * out_on, char * error_message,
                                    size_t error_message_size)
{
    if (out_on == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "read_on_off requires output pointer");
        return false;
    }

    bool on        = false;
    CHIP_ERROR err = gContext.ReadOnOff(node_id, endpoint, on);
    if (err == CHIP_NO_ERROR)
    {
        *out_on = on;
    }

    return HandleBridgeResult(err, error_message, error_message_size, "reading Matter on/off");
}

void rhythm_chip_bridge_shutdown(void)
{
    gContext.Shutdown();
}
