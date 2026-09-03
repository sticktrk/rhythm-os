#include "chip_bridge.h"

#include <app-common/zap-generated/cluster-objects.h>
#include <app/AttributePathParams.h>
#include <app/InteractionModelEngine.h>
#include <app/ReadClient.h>
#include <app/ReadPrepareParams.h>
#include <app/data-model/Decode.h>
#include <controller/CHIPCluster.h>
#include <controller/CHIPDeviceController.h>
#include <controller/CHIPDeviceControllerFactory.h>
#include <controller/ExampleOperationalCredentialsIssuer.h>
#include <controller/InvokeInteraction.h>
#include <controller/ExamplePersistentStorage.h>
#include <credentials/CHIPCert.h>
#include <credentials/GroupDataProviderImpl.h>
#include <credentials/PersistentStorageOpCertStore.h>
#include <credentials/attestation_verifier/DefaultDeviceAttestationVerifier.h>
#include <credentials/attestation_verifier/DeviceAttestationVerifier.h>
#include <credentials/attestation_verifier/FileAttestationTrustStore.h>
#include <crypto/CHIPCryptoPAL.h>
#include <crypto/RawKeySessionKeystore.h>
#include <data-model-providers/codegen/Instance.h>
#include <lib/core/CHIPCallback.h>
#include <lib/core/ErrorStr.h>
#include <lib/support/CodeUtils.h>
#include <lib/support/ScopedMemoryBuffer.h>
#include <platform/CHIPDeviceLayer.h>
#include <platform/TestOnlyCommissionableDataProvider.h>
#include <protocols/secure_channel/RendezvousParameters.h>
#include <setup_payload/ManualSetupPayloadParser.h>
#include <setup_payload/QRCodeSetupPayloadParser.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <set>
#include <sstream>
#include <string>
#include <string_view>
#include <utility>
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
using chip::Controller::OnNOCChainGeneration;
using chip::Controller::WiFiCredentials;

// Device operations are owned by chipd endpoint lanes. Their callback context
// stays alive until CHIP itself reports a terminal interaction-model result;
// no host wall-clock deadline is allowed to destroy controller-owned state or
// restart unrelated healthy endpoints.
constexpr std::chrono::seconds kCommissioningTimeout(180);
// Last-resort bound on a single blocking controller operation. It sits above
// everything this daemon legitimately waits for — the SDK's own operational
// discovery (~45s) plus CASE retries, and the 180s commissioning timeout below
// — so an ordinary slow endpoint, or an unrelated operation running while a
// commission is in flight, can never trip it. It only fires when the SDK
// violates its own timeout contract and leaves a caller wedged forever.
constexpr std::chrono::seconds kOperationWedgeDeadline(240);
constexpr EndpointId kRootEndpoint = kRootEndpointId;
constexpr VendorId kDefaultControllerVendorId = VendorId::TestVendor1;
constexpr const char * kBypassAttestationEnv = "RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION";
constexpr const char * kPaaTrustStorePathEnv = "RHYTHM_MATTER_PAA_TRUST_STORE_PATH";
constexpr const char * kAllowTestPaaEnv = "RHYTHM_MATTER_ALLOW_TEST_PAA";
constexpr KeysetId kRhythmGroupKeySetId = 0x5201;
constexpr uint64_t kRhythmGroupEpochStartTime = 1;
constexpr const char * kRhythmGroupEpochKeyStorageKey = "rhythm:matter-group-epoch-key:v1";
using RhythmIpk =
    std::array<uint8_t, chip::Credentials::GroupDataProvider::EpochKey::kLengthBytes>;
using RhythmGroupEpochKey = RhythmIpk;

std::optional<uint8_t> DecodeHexNibble(char value)
{
    if (value >= '0' && value <= '9')
    {
        return static_cast<uint8_t>(value - '0');
    }
    if (value >= 'a' && value <= 'f')
    {
        return static_cast<uint8_t>(value - 'a' + 10);
    }
    if (value >= 'A' && value <= 'F')
    {
        return static_cast<uint8_t>(value - 'A' + 10);
    }
    return std::nullopt;
}

CHIP_ERROR DecodeRhythmIpk(const char * ipkHex, RhythmIpk & out)
{
    VerifyOrReturnError(ipkHex != nullptr, CHIP_ERROR_INVALID_ARGUMENT);

    std::string_view hex(ipkHex);
    VerifyOrReturnError(hex.size() == out.size() * 2, CHIP_ERROR_INVALID_ARGUMENT);

    for (size_t i = 0; i < out.size(); ++i)
    {
        const auto high = DecodeHexNibble(hex[i * 2]);
        const auto low  = DecodeHexNibble(hex[i * 2 + 1]);
        VerifyOrReturnError(high.has_value() && low.has_value(), CHIP_ERROR_INVALID_ARGUMENT);
        out[i] = static_cast<uint8_t>((*high << 4) | *low);
    }

    return CHIP_NO_ERROR;
}

uint64_t ReadBigEndianUint64(const uint8_t * bytes)
{
    uint64_t value = 0;
    for (size_t i = 0; i < sizeof(uint64_t); ++i)
    {
        value = (value << 8) | bytes[i];
    }
    return value;
}

class RhythmOperationalCredentialsIssuer final : public ExampleOperationalCredentialsIssuer
{
public:
    void SetCommissioningIpk(const RhythmIpk & ipk) { mCommissioningIpk = ipk; }

    CHIP_ERROR GenerateNOCChain(const ByteSpan & csrElements, const ByteSpan & csrNonce,
                                const ByteSpan & attestationSignature, const ByteSpan & attestationChallenge,
                                const ByteSpan & DAC, const ByteSpan & PAI,
                                Callback::Callback<OnNOCChainGeneration> * onCompletion) override
    {
        VerifyOrReturnError(onCompletion != nullptr && onCompletion->mCall != nullptr, CHIP_ERROR_INVALID_ARGUMENT);

        IpkOverrideContext context{ onCompletion, &mCommissioningIpk };
        Callback::Callback<OnNOCChainGeneration> callback(&OnNOCChainGenerated, &context);
        return ExampleOperationalCredentialsIssuer::GenerateNOCChain(csrElements, csrNonce, attestationSignature,
                                                                     attestationChallenge, DAC, PAI, &callback);
    }

private:
    struct IpkOverrideContext
    {
        Callback::Callback<OnNOCChainGeneration> * original;
        const RhythmIpk * ipk;
    };

    static void OnNOCChainGenerated(void * rawContext, CHIP_ERROR status, const ByteSpan & noc, const ByteSpan & icac,
                                    const ByteSpan & rcac, Optional<Crypto::IdentityProtectionKeySpan> ignoredIpk,
                                    Optional<NodeId> adminSubject)
    {
        (void) ignoredIpk;

        auto * context = static_cast<IpkOverrideContext *>(rawContext);
        VerifyOrReturn(context != nullptr && context->original != nullptr && context->original->mCall != nullptr &&
                       context->ipk != nullptr);

        Crypto::IdentityProtectionKeySpan ipkSpan(*context->ipk);
        ChipLogProgress(Controller, "Rhythm Matter generated device NOC chain with configured fabric IPK");
        context->original->mCall(context->original->mContext, status, noc, icac, rcac, MakeOptional(ipkSpan), adminSubject);
    }

    RhythmIpk mCommissioningIpk = {};
};

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

bool EnvFlagEnabled(const char * name)
{
    const char * value = std::getenv(name);
    if (value == nullptr)
    {
        return false;
    }

    std::string normalized(value);
    std::transform(normalized.begin(), normalized.end(), normalized.begin(),
                   [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); });
    return normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on";
}

std::optional<std::string> EnvValue(const char * name)
{
    const char * value = std::getenv(name);
    if (value == nullptr || value[0] == '\0')
    {
        return std::nullopt;
    }
    return std::string(value);
}

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

template <typename T>
void AppendNumberArray(std::ostringstream & json, const std::vector<T> & values)
{
    json << '[';
    for (size_t i = 0; i < values.size(); ++i)
    {
        if (i > 0)
        {
            json << ',';
        }
        json << static_cast<uint64_t>(values[i]);
    }
    json << ']';
}

void AppendJsonString(std::ostringstream & json, std::string_view value)
{
    json << '"';
    for (char ch : value)
    {
        switch (ch)
        {
        case '"':
            json << "\\\"";
            break;
        case '\\':
            json << "\\\\";
            break;
        case '\n':
            json << "\\n";
            break;
        case '\r':
            json << "\\r";
            break;
        case '\t':
            json << "\\t";
            break;
        default:
            json << ch;
            break;
        }
    }
    json << '"';
}

void AppendJsonError(std::ostringstream & json, CHIP_ERROR err)
{
    AppendJsonString(json, chip::ErrorStr(err));
}

template <typename T>
void AppendRawJsonValue(std::ostringstream & json, T value)
{
    json << static_cast<uint64_t>(value);
}

void AppendRawJsonValue(std::ostringstream & json, bool value)
{
    json << (value ? "true" : "false");
}

template <typename T>
void AppendReadValue(std::ostringstream & json, const char * name, CHIP_ERROR err, T value)
{
    json << ",\"" << name << "\":";
    if (err == CHIP_NO_ERROR)
    {
        AppendRawJsonValue(json, value);
    }
    else
    {
        json << "null";
    }
}

template <typename T>
void AppendReadObject(std::ostringstream & json, const char * name, CHIP_ERROR err, T value, bool leadingComma)
{
    if (leadingComma)
    {
        json << ',';
    }
    json << '"' << name << "\":{\"ok\":" << (err == CHIP_NO_ERROR ? "true" : "false");
    if (err == CHIP_NO_ERROR)
    {
        json << ",\"value\":";
        AppendRawJsonValue(json, value);
    }
    else
    {
        json << ",\"error\":";
        AppendJsonError(json, err);
    }
    json << '}';
}

void AppendReadBoolObject(std::ostringstream & json, const char * name, CHIP_ERROR err, bool value, bool leadingComma)
{
    AppendReadObject(json, name, err, value, leadingComma);
}

template <typename NullableT>
void AppendNullableValue(std::ostringstream & json, CHIP_ERROR err, const NullableT & value)
{
    if (err == CHIP_NO_ERROR && !value.IsNull())
    {
        AppendRawJsonValue(json, value.Value());
    }
    else
    {
        json << "null";
    }
}

template <typename NullableT>
void AppendNullableReadObject(std::ostringstream & json, const char * name, CHIP_ERROR err, const NullableT & value,
                              bool leadingComma)
{
    if (leadingComma)
    {
        json << ',';
    }
    json << '"' << name << "\":{\"ok\":" << (err == CHIP_NO_ERROR ? "true" : "false");
    if (err == CHIP_NO_ERROR)
    {
        json << ",\"value\":";
        AppendNullableValue(json, err, value);
    }
    else
    {
        json << ",\"error\":";
        AppendJsonError(json, err);
    }
    json << '}';
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

bool IsReasonableColorTemperatureKelvin(uint16_t kelvin)
{
    return kelvin >= 1500 && kelvin <= 10000;
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
    // Bounded wait: if the CHIP event loop is wedged/dead, scheduled work
    // never runs and this blocks the (serial) RPC daemon forever. We cannot
    // simply return on timeout — `work` and the callback's captures live on
    // this stack frame, so a late-running callback would be a use-after-free.
    // Exiting is safe: rhythm-matter detects the dead daemon and respawns it,
    // turning an indefinite Matter outage into a ~10s recovery.
    if (!work.condition.wait_for(lock, std::chrono::seconds(60), [&work] { return work.done; }))
    {
        ChipLogError(Controller, "Matter event loop failed to run scheduled work within 60s; exiting for supervisor restart");
        std::_Exit(70); // EX_SOFTWARE; skip destructors that could also hang
    }
    return CHIP_NO_ERROR;
}

// Runs a cleanup action unless it is disarmed first. Used to release
// Matter-thread bookkeeping on every early return of a multi-step RPC.
template <typename Fn>
class ScopeGuard
{
public:
    explicit ScopeGuard(Fn fn) : mFn(std::move(fn)) {}
    ~ScopeGuard()
    {
        if (mArmed)
        {
            mFn();
        }
    }

    ScopeGuard(const ScopeGuard &)             = delete;
    ScopeGuard & operator=(const ScopeGuard &) = delete;

    void Disarm() { mArmed = false; }

private:
    Fn mFn;
    bool mArmed = true;
};

class BlockingOperationBase
{
public:
    CHIP_ERROR WaitForCompletion()
    {
        std::unique_lock<std::mutex> lock(mMutex);
        // The operation object and its callback captures live on the caller's
        // stack, so returning early would be a use-after-free once a late
        // callback runs. Waiting is therefore unbounded for every timeout the
        // SDK owns; the deadline below only catches an SDK that never reports
        // a terminal result at all, and exits for supervisor restart exactly
        // like the ScheduleWork wedge guard above.
        if (!mCondition.wait_for(lock, kOperationWedgeDeadline, [this] { return mDone; }))
        {
            ChipLogError(Controller,
                         "Matter %s operation did not report a terminal result within its wedge deadline; "
                         "exiting for supervisor restart",
                         mLabel);
            std::_Exit(70); // EX_SOFTWARE; skip destructors that could also hang
        }
        return mStatus;
    }

protected:
    /// Names the operation kind in the wedge-guard log. Set once at
    /// construction, before the operation is started.
    void SetOperationLabel(const char * label) { mLabel = label; }

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
    const char * mLabel = "controller";
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
    {
        SetOperationLabel("invoke-command");
    }

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
    {
        SetOperationLabel("read-attribute");
    }

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

template <typename AttributeInfo>
class BlockingWriteAttributeOperation final : public DeviceConnectionOperation
{
public:
    using ValueType = typename AttributeInfo::Type;

    BlockingWriteAttributeOperation(NodeId nodeId, EndpointId endpoint, const ValueType & value) :
        DeviceConnectionOperation(nodeId), mEndpoint(endpoint), mValue(value)
    {
        SetOperationLabel("write-attribute");
    }

protected:
    CHIP_ERROR HandleConnected(Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle) override
    {
        ClusterBase cluster(exchangeMgr, sessionHandle, mEndpoint);
        return cluster.template WriteAttribute<AttributeInfo>(mValue, this, OnSuccess, OnFailure, OnDone);
    }

private:
    static void OnSuccess(void * context)
    {
        (void) context;
    }

    static void OnFailure(void * context, CHIP_ERROR error)
    {
        auto * self = static_cast<BlockingWriteAttributeOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->RecordFailure(error);
    }

    static void OnDone(void * context)
    {
        auto * self = static_cast<BlockingWriteAttributeOperation *>(context);
        VerifyOrReturn(self != nullptr);
        self->Finish(self->CurrentStatus());
    }

    void RecordFailure(CHIP_ERROR error)
    {
        std::lock_guard<std::mutex> lock(mWriteMutex);
        if (mWriteStatus == CHIP_NO_ERROR)
        {
            mWriteStatus = error;
        }
    }

    CHIP_ERROR CurrentStatus()
    {
        std::lock_guard<std::mutex> lock(mWriteMutex);
        return mWriteStatus;
    }

    EndpointId mEndpoint;
    ValueType mValue;
    std::mutex mWriteMutex;
    CHIP_ERROR mWriteStatus = CHIP_NO_ERROR;
};

// Map a terminal CHIP error onto the coarse, identifier-free class Rust uses to
// pick a backoff. Only error constants this translation unit (or the core SDK
// header it already pulls in) is known to define are matched; anything else is
// reported as OTHER and the raw code travels alongside for diagnostics.
//
// New CHIP_ERROR constants referenced by this file: CHIP_ERROR_BUSY,
// CHIP_ERROR_NOT_CONNECTED, CHIP_ERROR_CONNECTION_CLOSED_UNEXPECTEDLY. If a
// release-host build fails to resolve one of them, it belongs here and nowhere
// else in the bridge — drop it to OTHER rather than guessing a replacement.
// There is deliberately no ADDRESS_RESOLUTION or CASE_SESSION mapping: both
// surface as CHIP_ERROR_TIMEOUT through OperationalSessionSetup, so the native
// side cannot tell them apart (see chip_bridge.h).
uint8_t ClassifySubscriptionFailure(CHIP_ERROR error)
{
    if (error == CHIP_ERROR_TIMEOUT)
    {
        return static_cast<uint8_t>(RHYTHM_CHIP_BRIDGE_SUB_FAIL_TIMEOUT);
    }
    if (error == CHIP_ERROR_BUSY)
    {
        return static_cast<uint8_t>(RHYTHM_CHIP_BRIDGE_SUB_FAIL_RESOURCE_BUSY);
    }
    if (error == CHIP_ERROR_CONNECTION_ABORTED || error == CHIP_ERROR_CONNECTION_CLOSED_UNEXPECTEDLY ||
        error == CHIP_ERROR_NOT_CONNECTED)
    {
        return static_cast<uint8_t>(RHYTHM_CHIP_BRIDGE_SUB_FAIL_PEER_CLOSED);
    }
    return static_cast<uint8_t>(RHYTHM_CHIP_BRIDGE_SUB_FAIL_OTHER);
}

class LightStateSubscriptionOperation final : public DeviceConnectionOperation, public app::ReadClient::Callback
{
public:
    using ReportFn =
        std::function<void(NodeId, EndpointId, uint32_t, uint32_t, uint8_t, bool, uint64_t)>;
    using TerminatedFn = std::function<void(NodeId, EndpointId, CHIP_ERROR)>;

    LightStateSubscriptionOperation(NodeId nodeId, EndpointId endpoint, uint16_t minIntervalSecs,
                                    uint16_t maxIntervalSecs, ReportFn onReport, TerminatedFn onTerminated) :
        DeviceConnectionOperation(nodeId),
        mNodeId(nodeId), mEndpoint(endpoint), mMinIntervalSecs(minIntervalSecs), mMaxIntervalSecs(maxIntervalSecs),
        mOnReport(std::move(onReport)), mOnTerminated(std::move(onTerminated)),
        mAttributePaths{ app::AttributePathParams(endpoint, OnOff::Id, OnOff::Attributes::OnOff::Id),
                         app::AttributePathParams(endpoint, LevelControl::Id,
                                                  LevelControl::Attributes::CurrentLevel::Id),
                         app::AttributePathParams(endpoint, ColorControl::Id,
                                                  ColorControl::Attributes::CurrentHue::Id),
                         app::AttributePathParams(endpoint, ColorControl::Id,
                                                  ColorControl::Attributes::CurrentSaturation::Id),
                         app::AttributePathParams(endpoint, ColorControl::Id, ColorControl::Attributes::CurrentX::Id),
                         app::AttributePathParams(endpoint, ColorControl::Id, ColorControl::Attributes::CurrentY::Id),
                         app::AttributePathParams(endpoint, ColorControl::Id,
                                                  ColorControl::Attributes::ColorTemperatureMireds::Id) }
    {
        SetOperationLabel("light-state-subscribe");
    }

    bool IsActive() const { return mActive.load(std::memory_order_acquire); }
    bool UsesIntervals(uint16_t minIntervalSecs, uint16_t maxIntervalSecs) const
    {
        return mMinIntervalSecs == minIntervalSecs && mMaxIntervalSecs == maxIntervalSecs;
    }
    void StopForReplacement()
    {
        // Destruction of ReadClient aborts its exchange and timers without
        // invoking OnDone, so a deliberate interval change is not reported as
        // a peer failure to Rust's retry worker.
        mReadClient = nullptr;
        mActive.store(false, std::memory_order_release);
    }

protected:
    CHIP_ERROR HandleConnected(Messaging::ExchangeManager & exchangeMgr, const SessionHandle & sessionHandle) override
    {
        app::ReadPrepareParams params(sessionHandle);
        params.mpAttributePathParamsList    = mAttributePaths.data();
        params.mAttributePathParamsListSize = mAttributePaths.size();
        params.mMinIntervalFloorSeconds     = mMinIntervalSecs;
        params.mMaxIntervalCeilingSeconds   = mMaxIntervalSecs;
        params.mKeepSubscriptions           = true;
        params.mIsFabricFiltered            = true;

        mReadClient = Platform::MakeUnique<app::ReadClient>(
            app::InteractionModelEngine::GetInstance(), &exchangeMgr, *this,
            app::ReadClient::InteractionType::Subscribe);
        VerifyOrReturnError(mReadClient != nullptr, CHIP_ERROR_NO_MEMORY);

        // SendRequest intentionally leaves automatic re-subscription disabled.
        // Rust owns retry timing and re-enters through SubscribeOnOff after a
        // terminal subscription failure.
        CHIP_ERROR err = mReadClient->SendRequest(params);
        if (err != CHIP_NO_ERROR)
        {
            mReadClient = nullptr;
        }
        return err;
    }

private:
    void NotifySubscriptionStillActive(const app::ReadClient & readClient) override
    {
        (void) readClient;
        mActive.store(true, std::memory_order_release);
        if (mOnReport)
        {
            // A possibly-empty ReportData is the authoritative liveness signal.
            mOnReport(mNodeId, mEndpoint, 0, 0, RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_SUBSCRIPTION_ALIVE, false, 0);
        }
    }

    void OnAttributeData(const app::ConcreteDataAttributePath & path, TLV::TLVReader * data,
                         const app::StatusIB & status) override
    {
        if (!status.IsSuccess() || data == nullptr)
        {
            return;
        }

        uint8_t valueType = 0;
        bool boolValue    = false;
        uint64_t unsignedValue = 0;
        if (path.mClusterId == OnOff::Id && path.mAttributeId == OnOff::Attributes::OnOff::Id)
        {
            bool value;
            if (data->Get(value) != CHIP_NO_ERROR)
            {
                return;
            }
            valueType = RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_BOOL;
            boolValue = value;
        }
        else if ((path.mClusterId == LevelControl::Id &&
                  path.mAttributeId == LevelControl::Attributes::CurrentLevel::Id) ||
                 (path.mClusterId == ColorControl::Id &&
                  (path.mAttributeId == ColorControl::Attributes::CurrentHue::Id ||
                   path.mAttributeId == ColorControl::Attributes::CurrentSaturation::Id)))
        {
            uint8_t value;
            if (data->Get(value) != CHIP_NO_ERROR)
            {
                // CurrentLevel is nullable; a null report carries no usable
                // observed value and is intentionally omitted.
                return;
            }
            valueType     = RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_U8;
            unsignedValue = value;
        }
        else if (path.mClusterId == ColorControl::Id &&
                 (path.mAttributeId == ColorControl::Attributes::CurrentX::Id ||
                  path.mAttributeId == ColorControl::Attributes::CurrentY::Id ||
                  path.mAttributeId == ColorControl::Attributes::ColorTemperatureMireds::Id))
        {
            uint16_t value;
            if (data->Get(value) != CHIP_NO_ERROR)
            {
                return;
            }
            valueType     = RHYTHM_CHIP_BRIDGE_ATTRIBUTE_VALUE_U16;
            unsignedValue = value;
        }
        else
        {
            return;
        }

        mActive.store(true, std::memory_order_release);
        if (mOnReport)
        {
            mOnReport(mNodeId, mEndpoint, path.mClusterId, path.mAttributeId, valueType, boolValue,
                      unsignedValue);
        }
    }

    void OnSubscriptionEstablished(SubscriptionId subscriptionId) override
    {
        (void) subscriptionId;
        mEstablished = true;
        mActive.store(true, std::memory_order_release);
        Finish(CHIP_NO_ERROR);
    }

    // Record only. Finishing here would let the waiting RPC thread destroy this
    // operation — and with it the ReadClient — while ReadClient::Close() is
    // still running on the Matter thread. The terminal handoff belongs in
    // OnDone(), which the SDK guarantees is the last thing Close() does.
    // No log line: Rust owns termination logging, with backoff.
    void OnError(CHIP_ERROR error) override
    {
        mLastError = error;
        mActive.store(false, std::memory_order_release);
    }

    void OnDone(app::ReadClient * readClient) override
    {
        (void) readClient;
        mActive.store(false, std::memory_order_release);
        // Destroying the ReadClient inside OnDone on the Matter thread is the
        // SDK-sanctioned pattern (ClusterBase/TypedReadCallback do the same):
        // the ReadClient never touches itself after invoking this callback.
        mReadClient = nullptr;

        const CHIP_ERROR error = (mLastError == CHIP_NO_ERROR) ? CHIP_ERROR_CONNECTION_ABORTED : mLastError;
        if (!mEstablished)
        {
            // Pre-establishment failure: the blocked SubscribeOnOff RPC reports
            // it. Finish() can free `this` on the waiting thread, so nothing
            // may touch members after this point.
            Finish(error);
            return;
        }

        if (mOnTerminated)
        {
            mOnTerminated(mNodeId, mEndpoint, error);
        }
    }

    NodeId mNodeId;
    EndpointId mEndpoint;
    uint16_t mMinIntervalSecs;
    uint16_t mMaxIntervalSecs;
    ReportFn mOnReport;
    TerminatedFn mOnTerminated;
    std::array<app::AttributePathParams, 7> mAttributePaths;
    Platform::UniquePtr<app::ReadClient> mReadClient;
    std::atomic<bool> mActive{ false };
    bool mEstablished      = false;
    CHIP_ERROR mLastError = CHIP_NO_ERROR;
};

// One live On/Off subscription plus the key it was requested for. Key and
// operation are kept in a single record so they can never desync, and the whole
// table is owned by the Matter thread.
struct LightStateSubscriptionEntry
{
    NodeId nodeId;
    EndpointId endpoint;
    std::unique_ptr<LightStateSubscriptionOperation> operation;
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
    CHIP_ERROR Init(const char * storagePath, const char * fabricId, uint64_t operationalFabricId, const char * ipkHex,
                    bool hasBleController, uint16_t bleController, uint16_t controllerVendorId)
    {
        std::lock_guard<std::mutex> lock(mMutex);

        const std::string requestedStorage = storagePath != nullptr ? storagePath : "";
        if (requestedStorage.empty())
        {
            return CHIP_ERROR_INVALID_ARGUMENT;
        }
        VerifyOrReturnError(operationalFabricId != 0, CHIP_ERROR_INVALID_ARGUMENT);

        RhythmIpk requestedIpk;
        ReturnErrorOnFailure(DecodeRhythmIpk(ipkHex, requestedIpk));
        const std::string requestedFabricId = fabricId != nullptr ? fabricId : "";

        if (mCommissioner != nullptr)
        {
            if (requestedStorage == mStoragePath)
            {
                VerifyOrReturnError(mControllerVendorId == controllerVendorId, CHIP_ERROR_INCORRECT_STATE);
                VerifyOrReturnError(mFabricId == requestedFabricId, CHIP_ERROR_INCORRECT_STATE);
                VerifyOrReturnError(mOperationalFabricId == operationalFabricId, CHIP_ERROR_INCORRECT_STATE);
                VerifyOrReturnError(mIpk == requestedIpk, CHIP_ERROR_INCORRECT_STATE);
                return CHIP_NO_ERROR;
            }
            return CHIP_ERROR_INCORRECT_STATE;
        }

        mStoragePath           = requestedStorage;
        mFabricId              = requestedFabricId;
        mOperationalFabricId   = operationalFabricId;
        mIpk                   = requestedIpk;
        mControllerVendorId    = controllerVendorId;

        if (!mFactoryInitialized)
        {
            ReturnErrorOnFailure(InitializeFactory(hasBleController, bleController));
        }

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err]() {
            mUnpairedNodes.clear();
            err = InitializeCommissioner();
        }));
        return err;
    }

    uint64_t CompressedFabricId()
    {
        std::lock_guard<std::mutex> lock(mMutex);
        return mCompressedFabricId;
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
        // The node is paired again: lift any unpair tombstone so subscriptions
        // can be installed for it.
        const NodeId commissionedNodeId = request.node_id;
        CHIP_ERROR tombstone = ExecuteOnMatterThread([this, commissionedNodeId]() { mUnpairedNodes.erase(commissionedNodeId); });
        if (tombstone != CHIP_NO_ERROR)
        {
            return tombstone;
        }
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
                        device.has_max_kelvin = IsReasonableColorTemperatureKelvin(device.max_kelvin);
                    }
                    if (ReadValueAttribute<ColorControl::Attributes::ColorTempPhysicalMaxMireds::TypeInfo>(nodeId, lightEndpoint,
                                                                                                          maxMireds) == CHIP_NO_ERROR)
                    {
                        device.min_kelvin     = MiredsToKelvin(maxMireds);
                        device.has_min_kelvin = IsReasonableColorTemperatureKelvin(device.min_kelvin);
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
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err, nodeId]() {
            err = mCommissioner->UnpairDevice(nodeId);
            if (err == CHIP_NO_ERROR)
            {
                RemoveLightStateSubscriptionsForNode(nodeId);
            }
        }));
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

    CHIP_ERROR ConfigureGroup(const rhythm_chip_bridge_group & group)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);
        VerifyOrReturnError(group.group_id != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(group.name != nullptr, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(group.members != nullptr || group.member_count == 0, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(std::strlen(group.name) <= CHIP_CONFIG_MAX_GROUP_NAME_LENGTH, CHIP_ERROR_INVALID_ARGUMENT);

        const GroupId groupId     = group.group_id;
        const std::string name    = group.name;
        const FabricIndex fabric  = mCommissioner->GetFabricIndex();
        CHIP_ERROR providerStatus = CHIP_NO_ERROR;
        ChipLogProgress(Controller, "Rhythm Matter group configure: group=%u name=%s members=%zu fabric=%u",
                        static_cast<unsigned>(groupId), name.c_str(), group.member_count, static_cast<unsigned>(fabric));
        ReturnErrorOnFailure(EnsureControllerRhythmGroupKeySet(fabric));
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, fabric, groupId, &name, &providerStatus]() {
            chip::Credentials::GroupDataProvider::GroupInfo groupInfo(groupId, name.c_str());
            providerStatus = mGroupDataProvider.SetGroupInfo(fabric, groupInfo);
            if (providerStatus != CHIP_NO_ERROR)
            {
                return;
            }
            providerStatus = mGroupDataProvider.SetGroupKey(fabric, groupId, kRhythmGroupKeySetId);
        }));
        ReturnErrorOnFailure(providerStatus);
        ChipLogProgress(Controller, "Rhythm Matter group key bound: group=%u keyset=%u fabric=%u",
                        static_cast<unsigned>(groupId), static_cast<unsigned>(kRhythmGroupKeySetId),
                        static_cast<unsigned>(fabric));

        for (size_t i = 0; i < group.member_count; ++i)
        {
            const NodeId nodeId       = group.members[i].node_id;
            const EndpointId endpoint = group.members[i].endpoint;

            ReturnErrorOnFailure(ProvisionGroupKeyOnDevice(nodeId, groupId));

            CHIP_ERROR endpointStatus = CHIP_NO_ERROR;
            ReturnErrorOnFailure(ExecuteOnMatterThread([this, fabric, groupId, endpoint, &endpointStatus]() {
                endpointStatus = mGroupDataProvider.AddEndpoint(fabric, groupId, endpoint);
            }));
            ReturnErrorOnFailure(endpointStatus);

            Groups::Commands::AddGroup::Type request;
            request.groupID   = groupId;
            request.groupName = chip::CharSpan::fromCharString(name.c_str());
            ReturnErrorOnFailure(InvokeCommand(nodeId, endpoint, request));
            ChipLogProgress(Controller, "Rhythm Matter group member added: group=%u node=" ChipLogFormatX64 " endpoint=%u",
                            static_cast<unsigned>(groupId), ChipLogValueX64(nodeId), static_cast<unsigned>(endpoint));
        }

        return CHIP_NO_ERROR;
    }

    CHIP_ERROR RemoveGroup(GroupId groupId, const rhythm_chip_bridge_group_member * members, size_t memberCount)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(members != nullptr || memberCount == 0, CHIP_ERROR_INVALID_ARGUMENT);

        ChipLogProgress(Controller, "Rhythm Matter group remove: group=%u members=%zu", static_cast<unsigned>(groupId),
                        memberCount);
        for (size_t i = 0; i < memberCount; ++i)
        {
            Groups::Commands::RemoveGroup::Type request;
            request.groupID = groupId;
            ReturnErrorOnFailure(InvokeCommand(members[i].node_id, members[i].endpoint, request));
            ChipLogProgress(Controller, "Rhythm Matter group member removed: group=%u node=" ChipLogFormatX64 " endpoint=%u",
                            static_cast<unsigned>(groupId), ChipLogValueX64(members[i].node_id),
                            static_cast<unsigned>(members[i].endpoint));
        }

        const FabricIndex fabric = mCommissioner->GetFabricIndex();
        CHIP_ERROR providerStatus = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, fabric, groupId, &providerStatus]() {
            providerStatus = RemoveGroupKey(fabric, groupId);
            if (providerStatus != CHIP_NO_ERROR && providerStatus != CHIP_ERROR_NOT_FOUND)
            {
                return;
            }

            providerStatus = mGroupDataProvider.RemoveGroupInfo(fabric, groupId);
            if (providerStatus == CHIP_ERROR_NOT_FOUND)
            {
                providerStatus = CHIP_NO_ERROR;
            }
        }));
        return providerStatus;
    }

    CHIP_ERROR SetGroupOnOff(GroupId groupId, bool on)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        if (on)
        {
            OnOff::Commands::On::Type request;
            CHIP_ERROR err = InvokeGroupCommand(groupId, request);
            if (err == CHIP_NO_ERROR)
            {
                ChipLogProgress(Controller, "Rhythm Matter group command: op=on_off group=%u on=true",
                                static_cast<unsigned>(groupId));
            }
            return err;
        }

        OnOff::Commands::Off::Type request;
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller, "Rhythm Matter group command: op=on_off group=%u on=false",
                            static_cast<unsigned>(groupId));
        }
        return err;
    }

    CHIP_ERROR IdentifyGroup(GroupId groupId, uint16_t durationSecs)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        Identify::Commands::Identify::Type request;
        request.identifyTime = durationSecs;
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller, "Rhythm Matter group command: op=identify group=%u duration_secs=%u",
                            static_cast<unsigned>(groupId), static_cast<unsigned>(durationSecs));
        }
        return err;
    }

    CHIP_ERROR SetGroupBrightness(GroupId groupId, uint8_t level, std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        LevelControl::Commands::MoveToLevelWithOnOff::Type request;
        request.level          = level;
        request.transitionTime = transitionMs.has_value() ? chip::app::DataModel::Nullable<uint16_t>(MillisecondsToTenths(*transitionMs))
                                                          : chip::app::DataModel::Nullable<uint16_t>();
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller, "Rhythm Matter group command: op=brightness group=%u level=%u transition_ms=%s",
                            static_cast<unsigned>(groupId), static_cast<unsigned>(level),
                            transitionMs.has_value() ? std::to_string(*transitionMs).c_str() : "none");
        }
        return err;
    }

    CHIP_ERROR SetGroupColorTemperature(GroupId groupId, uint16_t kelvin, std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(kelvin > 0, CHIP_ERROR_INVALID_ARGUMENT);

        ColorControl::Commands::MoveToColorTemperature::Type request;
        request.colorTemperatureMireds = KelvinToMireds(kelvin);
        request.transitionTime         = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller, "Rhythm Matter group command: op=color_temperature group=%u kelvin=%u transition_ms=%s",
                            static_cast<unsigned>(groupId), static_cast<unsigned>(kelvin),
                            transitionMs.has_value() ? std::to_string(*transitionMs).c_str() : "none");
        }
        return err;
    }

    CHIP_ERROR SetGroupXy(GroupId groupId, float x, float y, std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        ColorControl::Commands::MoveToColor::Type request;
        request.colorX         = XyToMatterCoordinate(x);
        request.colorY         = XyToMatterCoordinate(y);
        request.transitionTime = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller, "Rhythm Matter group command: op=xy group=%u x=%.4f y=%.4f transition_ms=%s",
                            static_cast<unsigned>(groupId), static_cast<double>(x), static_cast<double>(y),
                            transitionMs.has_value() ? std::to_string(*transitionMs).c_str() : "none");
        }
        return err;
    }

    CHIP_ERROR SetGroupHueSaturation(GroupId groupId, uint8_t hue, uint8_t saturation,
                                     std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(groupId != kUndefinedGroupId, CHIP_ERROR_INVALID_ARGUMENT);
        ColorControl::Commands::MoveToHueAndSaturation::Type request;
        request.hue             = hue;
        request.saturation      = saturation;
        request.transitionTime  = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        CHIP_ERROR err = InvokeGroupCommand(groupId, request);
        if (err == CHIP_NO_ERROR)
        {
            ChipLogProgress(Controller,
                            "Rhythm Matter group command: op=hue_saturation group=%u hue=%u saturation=%u transition_ms=%s",
                            static_cast<unsigned>(groupId), static_cast<unsigned>(hue), static_cast<unsigned>(saturation),
                            transitionMs.has_value() ? std::to_string(*transitionMs).c_str() : "none");
        }
        return err;
    }

    CHIP_ERROR IdentifyLight(NodeId nodeId, EndpointId endpoint, uint16_t durationSecs)
    {
        Identify::Commands::Identify::Type request;
        request.identifyTime = durationSecs;
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

    CHIP_ERROR RunLevelCommand(NodeId nodeId, EndpointId endpoint, uint8_t command, uint8_t levelOrStep, uint8_t stepMode,
                               std::optional<uint32_t> transitionMs)
    {
        const auto transition = transitionMs.has_value()
            ? chip::app::DataModel::Nullable<uint16_t>(MillisecondsToTenths(*transitionMs))
            : chip::app::DataModel::Nullable<uint16_t>();
        const auto mappedStepMode = stepMode == RHYTHM_CHIP_BRIDGE_LEVEL_STEP_MODE_DOWN
            ? LevelControl::StepModeEnum::kDown
            : LevelControl::StepModeEnum::kUp;

        switch (command)
        {
        case RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_MOVE_TO_LEVEL: {
            LevelControl::Commands::MoveToLevel::Type request;
            request.level          = levelOrStep;
            request.transitionTime = transition;
            request.optionsMask     = chip::BitMask<LevelControl::OptionsBitmap>();
            request.optionsOverride = chip::BitMask<LevelControl::OptionsBitmap>();
            return InvokeCommand(nodeId, endpoint, request);
        }
        case RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_MOVE_TO_LEVEL_WITH_ON_OFF: {
            LevelControl::Commands::MoveToLevelWithOnOff::Type request;
            request.level          = levelOrStep;
            request.transitionTime = transition;
            request.optionsMask     = chip::BitMask<LevelControl::OptionsBitmap>();
            request.optionsOverride = chip::BitMask<LevelControl::OptionsBitmap>();
            return InvokeCommand(nodeId, endpoint, request);
        }
        case RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_STEP: {
            LevelControl::Commands::Step::Type request;
            request.stepMode        = mappedStepMode;
            request.stepSize        = levelOrStep;
            request.transitionTime  = transition;
            request.optionsMask     = chip::BitMask<LevelControl::OptionsBitmap>();
            request.optionsOverride = chip::BitMask<LevelControl::OptionsBitmap>();
            return InvokeCommand(nodeId, endpoint, request);
        }
        case RHYTHM_CHIP_BRIDGE_LEVEL_COMMAND_STEP_WITH_ON_OFF: {
            LevelControl::Commands::StepWithOnOff::Type request;
            request.stepMode        = mappedStepMode;
            request.stepSize        = levelOrStep;
            request.transitionTime  = transition;
            request.optionsMask     = chip::BitMask<LevelControl::OptionsBitmap>();
            request.optionsOverride = chip::BitMask<LevelControl::OptionsBitmap>();
            return InvokeCommand(nodeId, endpoint, request);
        }
        default:
            return CHIP_ERROR_INVALID_ARGUMENT;
        }
    }

    CHIP_ERROR SetColorTemperature(NodeId nodeId, EndpointId endpoint, uint16_t kelvin, std::optional<uint32_t> transitionMs)
    {
        VerifyOrReturnError(kelvin > 0, CHIP_ERROR_INVALID_ARGUMENT);

        ColorControl::Commands::MoveToColorTemperature::Type request;
        request.colorTemperatureMireds = KelvinToMireds(kelvin);
        request.transitionTime         = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR SetXy(NodeId nodeId, EndpointId endpoint, float x, float y, std::optional<uint32_t> transitionMs)
    {
        ColorControl::Commands::MoveToColor::Type request;
        request.colorX         = XyToMatterCoordinate(x);
        request.colorY         = XyToMatterCoordinate(y);
        request.transitionTime = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR SetHueSaturation(NodeId nodeId, EndpointId endpoint, uint8_t hue, uint8_t saturation,
                                std::optional<uint32_t> transitionMs)
    {
        ColorControl::Commands::MoveToHueAndSaturation::Type request;
        request.hue             = hue;
        request.saturation      = saturation;
        request.transitionTime  = transitionMs.has_value() ? MillisecondsToTenths(*transitionMs) : 0;
        request.optionsMask.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        request.optionsOverride.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        return InvokeCommand(nodeId, endpoint, request);
    }

    CHIP_ERROR WriteColorControlOptions(NodeId nodeId, EndpointId endpoint, bool executeIfOff)
    {
        chip::BitMask<ColorControl::OptionsBitmap> options;
        ReturnErrorOnFailure(
            ReadValueAttribute<ColorControl::Attributes::Options::TypeInfo>(nodeId, endpoint, options));
        if (executeIfOff)
        {
            options.Set(ColorControl::OptionsBitmap::kExecuteIfOff);
        }
        else
        {
            options.Clear(ColorControl::OptionsBitmap::kExecuteIfOff);
        }
        return WriteAttribute<ColorControl::Attributes::Options::TypeInfo>(nodeId, endpoint, options);
    }

    CHIP_ERROR ReadOnOff(NodeId nodeId, EndpointId endpoint, bool & on)
    {
        return ReadValueAttribute<OnOff::Attributes::OnOff::TypeInfo>(nodeId, endpoint, on);
    }

    CHIP_ERROR ReadLightCapabilitySnapshot(NodeId nodeId, EndpointId endpoint, std::string & out)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        std::vector<EndpointId> endpoints;
        CHIP_ERROR endpointErr =
            ReadEndpointListAttribute<Descriptor::Attributes::PartsList::TypeInfo>(nodeId, kRootEndpoint, endpoints);
        if (endpointErr != CHIP_NO_ERROR || endpoints.empty())
        {
            endpoints = { endpoint };
        }
        if (std::find(endpoints.begin(), endpoints.end(), kRootEndpoint) == endpoints.end())
        {
            endpoints.insert(endpoints.begin(), kRootEndpoint);
        }

        std::vector<ClusterId> serverClusters;
        std::vector<ClusterId> clientClusters;
        std::vector<Descriptor::Structs::DeviceTypeStruct::DecodableType> deviceTypes;
        (void) ReadClusterListAttribute<Descriptor::Attributes::ServerList::TypeInfo>(nodeId, endpoint, serverClusters);
        (void) ReadClusterListAttribute<Descriptor::Attributes::ClientList::TypeInfo>(nodeId, endpoint, clientClusters);
        (void) ReadDeviceTypeListAttribute(nodeId, endpoint, deviceTypes);

        std::vector<CommandId> onOffAccepted;
        std::vector<AttributeId> onOffAttributes;
        uint32_t onOffFeatureMap = 0;
        (void) ReadCommandListAttribute<OnOff::Attributes::AcceptedCommandList::TypeInfo>(nodeId, endpoint, onOffAccepted);
        (void) ReadAttributeIdListAttribute<OnOff::Attributes::AttributeList::TypeInfo>(nodeId, endpoint, onOffAttributes);
        (void) ReadValueAttribute<OnOff::Attributes::FeatureMap::TypeInfo>(nodeId, endpoint, onOffFeatureMap);

        std::vector<CommandId> levelAccepted;
        std::vector<AttributeId> levelAttributes;
        uint32_t levelFeatureMap = 0;
        chip::app::DataModel::Nullable<uint8_t> currentLevel;
        (void) ReadCommandListAttribute<LevelControl::Attributes::AcceptedCommandList::TypeInfo>(nodeId, endpoint,
                                                                                                 levelAccepted);
        (void) ReadAttributeIdListAttribute<LevelControl::Attributes::AttributeList::TypeInfo>(nodeId, endpoint,
                                                                                               levelAttributes);
        (void) ReadValueAttribute<LevelControl::Attributes::FeatureMap::TypeInfo>(nodeId, endpoint, levelFeatureMap);
        CHIP_ERROR currentLevelErr =
            ReadValueAttribute<LevelControl::Attributes::CurrentLevel::TypeInfo>(nodeId, endpoint, currentLevel);

        std::vector<CommandId> colorAccepted;
        std::vector<AttributeId> colorAttributes;
        uint32_t colorFeatureMap = 0;
        chip::BitMask<ColorControl::ColorCapabilitiesBitmap> colorCapabilities;
        chip::BitMask<ColorControl::OptionsBitmap> colorOptions;
        uint16_t minMireds = 0;
        uint16_t maxMireds = 0;
        uint16_t currentX = 0;
        uint16_t currentY = 0;
        uint8_t currentHue = 0;
        uint8_t currentSaturation = 0;
        (void) ReadCommandListAttribute<ColorControl::Attributes::AcceptedCommandList::TypeInfo>(nodeId, endpoint,
                                                                                                 colorAccepted);
        (void) ReadAttributeIdListAttribute<ColorControl::Attributes::AttributeList::TypeInfo>(nodeId, endpoint,
                                                                                               colorAttributes);
        (void) ReadValueAttribute<ColorControl::Attributes::FeatureMap::TypeInfo>(nodeId, endpoint, colorFeatureMap);
        CHIP_ERROR colorCapabilitiesErr =
            ReadValueAttribute<ColorControl::Attributes::ColorCapabilities::TypeInfo>(nodeId, endpoint, colorCapabilities);
        CHIP_ERROR colorOptionsErr =
            ReadValueAttribute<ColorControl::Attributes::Options::TypeInfo>(nodeId, endpoint, colorOptions);
        CHIP_ERROR minMiredsErr =
            ReadValueAttribute<ColorControl::Attributes::ColorTempPhysicalMinMireds::TypeInfo>(nodeId, endpoint, minMireds);
        CHIP_ERROR maxMiredsErr =
            ReadValueAttribute<ColorControl::Attributes::ColorTempPhysicalMaxMireds::TypeInfo>(nodeId, endpoint, maxMireds);
        CHIP_ERROR currentXErr =
            ReadValueAttribute<ColorControl::Attributes::CurrentX::TypeInfo>(nodeId, endpoint, currentX);
        CHIP_ERROR currentYErr =
            ReadValueAttribute<ColorControl::Attributes::CurrentY::TypeInfo>(nodeId, endpoint, currentY);
        CHIP_ERROR currentHueErr =
            ReadValueAttribute<ColorControl::Attributes::CurrentHue::TypeInfo>(nodeId, endpoint, currentHue);
        CHIP_ERROR currentSaturationErr =
            ReadValueAttribute<ColorControl::Attributes::CurrentSaturation::TypeInfo>(nodeId, endpoint, currentSaturation);

        std::ostringstream json;
        json << "{\"node_id\":" << static_cast<uint64_t>(nodeId) << ",\"selected_endpoint\":"
             << static_cast<unsigned>(endpoint) << ",\"raw_attribute_reads_available\":true";
        json << ",\"endpoint_list\":";
        AppendNumberArray(json, endpoints);
        json << ",\"server_clusters\":";
        AppendNumberArray(json, serverClusters);
        json << ",\"client_clusters\":";
        AppendNumberArray(json, clientClusters);
        json << ",\"device_type_list\":[";
        for (size_t i = 0; i < deviceTypes.size(); ++i)
        {
            if (i > 0)
            {
                json << ',';
            }
            json << "{\"device_type\":" << static_cast<uint32_t>(deviceTypes[i].deviceType)
                 << ",\"revision\":" << static_cast<uint16_t>(deviceTypes[i].revision) << '}';
        }
        json << ']';

        json << ",\"accepted_command_lists\":{\"onoff\":";
        AppendNumberArray(json, onOffAccepted);
        json << ",\"level_control\":";
        AppendNumberArray(json, levelAccepted);
        json << ",\"color_control\":";
        AppendNumberArray(json, colorAccepted);
        json << '}';

        json << ",\"attribute_lists\":{\"onoff\":";
        AppendNumberArray(json, onOffAttributes);
        json << ",\"level_control\":";
        AppendNumberArray(json, levelAttributes);
        json << ",\"color_control\":";
        AppendNumberArray(json, colorAttributes);
        json << '}';

        json << ",\"onoff\":{\"feature_map\":" << onOffFeatureMap << ",\"accepted_command_list\":";
        AppendNumberArray(json, onOffAccepted);
        json << ",\"attribute_list\":";
        AppendNumberArray(json, onOffAttributes);
        json << '}';

        json << ",\"level_control\":{\"feature_map\":" << levelFeatureMap << ",\"accepted_command_list\":";
        AppendNumberArray(json, levelAccepted);
        json << ",\"attribute_list\":";
        AppendNumberArray(json, levelAttributes);
        json << ",\"current_level\":";
        AppendNullableValue(json, currentLevelErr, currentLevel);
        json << '}';

        json << ",\"color_control\":{\"feature_map\":" << colorFeatureMap << ",\"color_capabilities\":";
        if (colorCapabilitiesErr == CHIP_NO_ERROR)
        {
            json << colorCapabilities.Raw();
        }
        else
        {
            json << "null";
        }
        json << ",\"accepted_command_list\":";
        AppendNumberArray(json, colorAccepted);
        json << ",\"attribute_list\":";
        AppendNumberArray(json, colorAttributes);
        AppendReadValue(json, "color_temp_physical_min_mireds", minMiredsErr, minMireds);
        AppendReadValue(json, "color_temp_physical_max_mireds", maxMiredsErr, maxMireds);
        AppendReadValue(json, "current_x", currentXErr, currentX);
        AppendReadValue(json, "current_y", currentYErr, currentY);
        AppendReadValue(json, "current_hue", currentHueErr, currentHue);
        AppendReadValue(json, "current_saturation", currentSaturationErr, currentSaturation);
        json << ",\"options\":";
        if (colorOptionsErr == CHIP_NO_ERROR)
        {
            json << static_cast<unsigned>(colorOptions.Raw());
        }
        else
        {
            json << "null";
        }
        json << '}';
        json << '}';
        out = json.str();
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR ReadLightStateJson(NodeId nodeId, EndpointId endpoint, std::string & out)
    {
        bool on = false;
        chip::app::DataModel::Nullable<uint8_t> currentLevel;
        uint16_t colorTemperatureMireds = 0;
        uint16_t currentX = 0;
        uint16_t currentY = 0;
        uint8_t currentHue = 0;
        uint8_t currentSaturation = 0;

        CHIP_ERROR onErr = ReadValueAttribute<OnOff::Attributes::OnOff::TypeInfo>(nodeId, endpoint, on);
        CHIP_ERROR levelErr =
            ReadValueAttribute<LevelControl::Attributes::CurrentLevel::TypeInfo>(nodeId, endpoint, currentLevel);
        CHIP_ERROR colorTemperatureErr =
            ReadValueAttribute<ColorControl::Attributes::ColorTemperatureMireds::TypeInfo>(nodeId, endpoint,
                                                                                          colorTemperatureMireds);
        CHIP_ERROR xErr = ReadValueAttribute<ColorControl::Attributes::CurrentX::TypeInfo>(nodeId, endpoint, currentX);
        CHIP_ERROR yErr = ReadValueAttribute<ColorControl::Attributes::CurrentY::TypeInfo>(nodeId, endpoint, currentY);
        CHIP_ERROR hueErr = ReadValueAttribute<ColorControl::Attributes::CurrentHue::TypeInfo>(nodeId, endpoint, currentHue);
        CHIP_ERROR saturationErr =
            ReadValueAttribute<ColorControl::Attributes::CurrentSaturation::TypeInfo>(nodeId, endpoint, currentSaturation);

        std::ostringstream json;
        json << '{';
        AppendReadBoolObject(json, "onoff", onErr, on, false);
        AppendNullableReadObject(json, "current_level", levelErr, currentLevel, true);
        AppendReadObject(json, "color_temperature_mireds", colorTemperatureErr, colorTemperatureMireds, true);
        AppendReadObject(json, "current_x", xErr, currentX, true);
        AppendReadObject(json, "current_y", yErr, currentY, true);
        AppendReadObject(json, "current_hue", hueErr, currentHue, true);
        AppendReadObject(json, "current_saturation", saturationErr, currentSaturation, true);
        json << '}';
        out = json.str();
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR SubscribeOnOff(const rhythm_chip_bridge_subscription_target * targets, size_t targetCount,
                              uint16_t minIntervalSecs, uint16_t maxIntervalSecs, bool replaceExisting = false)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);
        VerifyOrReturnError(targets != nullptr || targetCount == 0, CHIP_ERROR_INVALID_ARGUMENT);
        VerifyOrReturnError(maxIntervalSecs >= minIntervalSecs, CHIP_ERROR_INVALID_ARGUMENT);
        // One target per call. Every target costs Matter-thread hops plus a
        // blocking subscribe, and rhythm-matter deliberately drives one
        // endpoint at a time under its shared work budget; a batch would sit
        // unbounded inside a single RPC. An empty request stays a no-op.
        VerifyOrReturnError(targetCount <= 1, CHIP_ERROR_INVALID_ARGUMENT);

        // The subscription table is owned by the Matter thread: every read and
        // every mutation (including ReadClient destruction) is hopped there, so
        // a concurrent Decommission cannot race this RPC thread now that the
        // service no longer serializes SubscribeOnOff behind the lifecycle lock.
        // Prune terminal non-resubscribing clients before deciding whether a
        // target already has a live Rust-owned subscription.
        ReturnErrorOnFailure(ExecuteOnMatterThread([this]() { PruneInactiveLightStateSubscriptions(); }));

        for (size_t i = 0; i < targetCount; ++i)
        {
            const NodeId nodeId       = targets[i].node_id;
            const EndpointId endpoint = targets[i].endpoint;
            const auto key            = std::make_pair(nodeId, endpoint);

            // Check-and-reserve in one Matter-thread hop. Without the
            // reservation two concurrent callers (SubscribeOnOff racing the
            // post-commission subscribe) could both observe "not subscribed"
            // and install a second ReadClient for the same endpoint, which
            // mKeepSubscriptions=true would keep alive on the peer.
            bool skip = false;
            ReturnErrorOnFailure(ExecuteOnMatterThread([this, nodeId, endpoint, minIntervalSecs, maxIntervalSecs,
                                                        replaceExisting, key, &skip]() {
                auto existing = std::find_if(
                    mLightStateSubscriptions.begin(), mLightStateSubscriptions.end(),
                    [nodeId, endpoint](const LightStateSubscriptionEntry & entry) {
                        return entry.nodeId == nodeId && entry.endpoint == endpoint && entry.operation != nullptr &&
                            entry.operation->IsActive();
                    });
                if (existing != mLightStateSubscriptions.end())
                {
                    if (!replaceExisting || existing->operation->UsesIntervals(minIntervalSecs, maxIntervalSecs))
                    {
                        skip = true;
                        return;
                    }
                    existing->operation->StopForReplacement();
                    mLightStateSubscriptions.erase(existing);
                }
                skip = mPendingLightStateSubscriptions.count(key) != 0;
                if (!skip)
                {
                    mPendingLightStateSubscriptions.insert(key);
                }
            }));
            if (skip)
            {
                continue;
            }

            // Releases the reservation on every early return below. Disarmed
            // once the bookkeeping hop has taken ownership of the operation.
            ScopeGuard releaseReservation([this, key]() {
                CHIP_ERROR ignored =
                    ExecuteOnMatterThread([this, key]() { mPendingLightStateSubscriptions.erase(key); });
                (void) ignored;
            });

            auto subscription = std::make_unique<LightStateSubscriptionOperation>(
                nodeId, endpoint, minIntervalSecs, maxIntervalSecs,
                [this](NodeId reportNodeId, EndpointId reportEndpoint, uint32_t clusterId, uint32_t attributeId,
                       uint8_t valueType, bool boolValue, uint64_t unsignedValue) {
                    this->QueueLightStateReport(reportNodeId, reportEndpoint, clusterId, attributeId, valueType,
                                                boolValue, unsignedValue);
                },
                [this](NodeId terminatedNodeId, EndpointId terminatedEndpoint, CHIP_ERROR error) {
                    this->QueueSubscriptionTermination(terminatedNodeId, terminatedEndpoint, error);
                });
            // On failure the operation is destroyed here, on the RPC thread.
            // That is safe because every failing path releases the ReadClient
            // first: SendRequest failures clear it inline, and OnDone() clears
            // it on the Matter thread before finishing the wait.
            ReturnErrorOnFailure(RunConnectionOperation(*subscription));

            const CHIP_ERROR bookkeeping = ExecuteOnMatterThread([this, nodeId, endpoint, key, &subscription]() {
                mPendingLightStateSubscriptions.erase(key);
                if (subscription == nullptr)
                {
                    return;
                }
                // Established, then died before we got here (its termination is
                // already queued), or the node was unpaired while we were
                // subscribing: drop the operation here, on the Matter thread.
                if (!subscription->IsActive() || mUnpairedNodes.count(nodeId) != 0)
                {
                    subscription.reset();
                    return;
                }
                mLightStateSubscriptions.push_back(
                    LightStateSubscriptionEntry{ nodeId, endpoint, std::move(subscription) });
            });
            if (bookkeeping != CHIP_NO_ERROR)
            {
                // The subscription is established and still owns a live
                // ReadClient that only the Matter thread may destroy — and the
                // Matter thread is unreachable. Leak it deliberately rather
                // than destroying it here; the process is on its way down.
                (void) subscription.release();
                ChipLogError(Controller,
                             "Matter thread unreachable while recording an established subscription; leaking one "
                             "ReadClient instead of destroying it off-thread");
                return bookkeeping;
            }
            releaseReservation.Disarm();
        }

        return CHIP_NO_ERROR;
    }

    size_t DrainAttributeReports(rhythm_chip_bridge_attribute_report * reports, size_t reportsCapacity)
    {
        if (reports == nullptr || reportsCapacity == 0)
        {
            return 0;
        }

        std::lock_guard<std::mutex> lock(mReportMutex);
        const size_t count = std::min(reportsCapacity, mAttributeReports.size());
        for (size_t i = 0; i < count; ++i)
        {
            reports[i] = mAttributeReports[i];
        }
        mAttributeReports.erase(mAttributeReports.begin(), mAttributeReports.begin() + count);
        return count;
    }

    size_t DrainSubscriptionTerminations(rhythm_chip_bridge_subscription_termination * terminations,
                                         size_t terminationsCapacity)
    {
        if (terminations == nullptr || terminationsCapacity == 0)
        {
            return 0;
        }

        std::lock_guard<std::mutex> lock(mReportMutex);
        const size_t count = std::min(terminationsCapacity, mSubscriptionTerminations.size());
        for (size_t i = 0; i < count; ++i)
        {
            terminations[i] = mSubscriptionTerminations[i];
        }
        mSubscriptionTerminations.erase(mSubscriptionTerminations.begin(), mSubscriptionTerminations.begin() + count);
        return count;
    }

    void Shutdown()
    {
        std::lock_guard<std::mutex> lock(mMutex);

        if (!mFactoryInitialized)
        {
            return;
        }

        CHIP_ERROR teardown = ExecuteOnMatterThread([this]() {
            mLightStateSubscriptions.clear();
            mPendingLightStateSubscriptions.clear();
            mUnpairedNodes.clear();
            if (mCommissioner != nullptr)
            {
                mCommissioner->Shutdown();
                mCommissioner.reset();
            }
        });
        if (teardown != CHIP_NO_ERROR)
        {
            ChipLogError(Controller, "Matter controller teardown could not reach the Matter thread: %" CHIP_ERROR_FORMAT,
                         teardown.Format());
        }

        {
            std::lock_guard<std::mutex> reportLock(mReportMutex);
            mAttributeReports.clear();
            mSubscriptionTerminations.clear();
        }

        if (mEventLoopStarted)
        {
            chip::DeviceLayer::PlatformMgr().StopEventLoopTask();
            mEventLoopStarted = false;
        }

        DeviceControllerFactory::GetInstance().ReleaseSystemState();
        DeviceControllerFactory::GetInstance().Shutdown();

        mFactoryInitialized = false;
        mStorage.reset();
        mPaaTrustStore.reset();
        mStoragePath.clear();
        mFabricId.clear();
        mOperationalFabricId = 0;
        mCompressedFabricId  = 0;
        mIpk                 = {};
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

        chip::Credentials::DeviceAttestationVerifier * dacVerifier = nullptr;
        static BypassAttestationVerifier sBypassVerifier;
        if (EnvFlagEnabled(kBypassAttestationEnv))
        {
            // DEV-ONLY: skip Matter device attestation only when explicitly
            // requested for bring-up against devices whose PAA is missing from
            // the configured trust store.
            dacVerifier = &sBypassVerifier;
        }
        else if (auto paaTrustStorePath = EnvValue(kPaaTrustStorePathEnv))
        {
            mPaaTrustStore = std::make_unique<chip::Credentials::FileAttestationTrustStore>(
                paaTrustStorePath->c_str());
            VerifyOrReturnError(mPaaTrustStore->IsInitialized() && mPaaTrustStore->paaCount() > 0,
                                CHIP_ERROR_INVALID_ARGUMENT);
            dacVerifier = chip::Credentials::GetDefaultDACVerifier(mPaaTrustStore.get(),
                                                                    /* revocationDelegate = */ nullptr);
            VerifyOrReturnError(dacVerifier != nullptr, CHIP_ERROR_INCORRECT_STATE);
        }
        else if (EnvFlagEnabled(kAllowTestPaaEnv))
        {
            // DEV-ONLY: the SDK test PAA store accepts development devices.
            // Production must provide RHYTHM_MATTER_PAA_TRUST_STORE_PATH.
            dacVerifier = chip::Credentials::GetDefaultDACVerifier(
                chip::Credentials::GetTestAttestationTrustStore(),
                /* revocationDelegate = */ nullptr);
            VerifyOrReturnError(dacVerifier != nullptr, CHIP_ERROR_INCORRECT_STATE);
        }
        else
        {
            ChipLogError(Controller,
                         "Matter PAA trust store is required. Set %s to a directory of PAA DER certificates, or set %s=1 for "
                         "development devices.",
                         kPaaTrustStorePathEnv, kAllowTestPaaEnv);
            return CHIP_ERROR_INVALID_ARGUMENT;
        }
        chip::Credentials::SetDeviceAttestationVerifier(dacVerifier);

        auto commissioner = std::make_unique<DeviceCommissioner>();

        SetupParams commissionerParams;
        commissionerParams.deviceAttestationVerifier      = dacVerifier;
        commissionerParams.operationalCredentialsDelegate = &mOperationalCredentialsIssuer;
        commissionerParams.pairingDelegate               = &mPairingDelegate;
        commissionerParams.controllerVendorId            = static_cast<VendorId>(mControllerVendorId);

        ReturnErrorOnFailure(mOperationalKeypair.Initialize(chip::Crypto::ECPKeyTarget::ECDSA));
        commissionerParams.operationalKeypair = &mOperationalKeypair;

        mOperationalCredentialsIssuer.SetCommissioningIpk(mIpk);
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
            mStorage->GetLocalNodeId(), mOperationalFabricId, mStorage->GetCommissionerCATs(),
            mOperationalKeypair.Pubkey(), rcacSpan, icacSpan, nocSpan));

        ReturnErrorOnFailure(ValidateExistingFabricCompatibility(chip::ByteSpan(rcacSpan.data(), rcacSpan.size())));

        commissionerParams.controllerNOC  = nocSpan;
        commissionerParams.controllerICAC = icacSpan;
        commissionerParams.controllerRCAC = rcacSpan;

        ReturnErrorOnFailure(DeviceControllerFactory::GetInstance().SetupCommissioner(commissionerParams, *commissioner));

        uint8_t compressedFabricId[sizeof(uint64_t)] = { 0 };
        chip::MutableByteSpan compressedFabricIdSpan(compressedFabricId);
        ReturnErrorOnFailure(commissioner->GetCompressedFabricIdBytes(compressedFabricIdSpan));
        VerifyOrReturnError(compressedFabricIdSpan.size() == sizeof(uint64_t), CHIP_ERROR_INCORRECT_STATE);
        mCompressedFabricId = ReadBigEndianUint64(compressedFabricId);

        const chip::ByteSpan rhythmIpk(mIpk.data(), mIpk.size());
        ReturnErrorOnFailure(chip::Credentials::SetSingleIpkEpochKey(
            &mGroupDataProvider, commissioner->GetFabricIndex(), rhythmIpk, compressedFabricIdSpan));

        mCommissioner = std::move(commissioner);
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR ValidateExistingFabricCompatibility(const chip::ByteSpan & requestedRcac)
    {
        const auto * systemState = DeviceControllerFactory::GetInstance().GetSystemState();
        VerifyOrReturnError(systemState != nullptr, CHIP_ERROR_INCORRECT_STATE);

        const auto * fabricTable = systemState->Fabrics();
        VerifyOrReturnError(fabricTable != nullptr, CHIP_ERROR_INCORRECT_STATE);
        if (fabricTable->FabricCount() == 0)
        {
            return CHIP_NO_ERROR;
        }

        uint8_t requestedRootBuffer[chip::Credentials::kMaxCHIPCertLength] = { 0 };
        chip::MutableByteSpan requestedRootChip(requestedRootBuffer);
        ReturnErrorOnFailure(chip::Credentials::ConvertX509CertToChipCert(requestedRcac, requestedRootChip));

        chip::Credentials::P256PublicKeySpan requestedRootPublicKeySpan;
        ReturnErrorOnFailure(chip::Credentials::ExtractPublicKeyFromChipCert(requestedRootChip, requestedRootPublicKeySpan));
        chip::Crypto::P256PublicKey requestedRootPublicKey{ requestedRootPublicKeySpan };

        bool foundMatchingFabric = false;
        bool foundIncompatibleFabric = false;
        for (const auto & fabric : *fabricTable)
        {
            chip::Crypto::P256PublicKey storedRootPublicKey;
            CHIP_ERROR rootErr = fabric.FetchRootPubkey(storedRootPublicKey);
            const bool matchesRequestedFabric =
                rootErr == CHIP_NO_ERROR && fabric.GetFabricId() == mOperationalFabricId &&
                storedRootPublicKey.Matches(requestedRootPublicKey);
            foundMatchingFabric = foundMatchingFabric || matchesRequestedFabric;
            foundIncompatibleFabric = foundIncompatibleFabric || !matchesRequestedFabric;

            if (!matchesRequestedFabric)
            {
                ChipLogError(Controller,
                             "Rhythm Matter controller storage has incompatible fabric index 0x%x fabricId=0x" ChipLogFormatX64
                             " compressedFabricId=0x" ChipLogFormatX64 " nodeId=0x" ChipLogFormatX64
                             " rootStatus=%" CHIP_ERROR_FORMAT,
                             static_cast<unsigned>(fabric.GetFabricIndex()), ChipLogValueX64(fabric.GetFabricId()),
                             ChipLogValueX64(fabric.GetCompressedFabricId()), ChipLogValueX64(fabric.GetNodeId()),
                             rootErr.Format());
            }
        }

        if (foundMatchingFabric && !foundIncompatibleFabric)
        {
            return CHIP_NO_ERROR;
        }

        ChipLogError(Controller,
                     "Rhythm Matter controller storage is incompatible with requested fabricId=0x" ChipLogFormatX64
                     "; reset /data/matter or restore the matching fabric identity instead of adding another local fabric",
                     ChipLogValueX64(mOperationalFabricId));
        return CHIP_ERROR_INCORRECT_STATE;
    }

    template <typename OperationT>
    CHIP_ERROR RunConnectionOperation(OperationT & operation)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err, &operation]() { err = operation.Start(*mCommissioner); }));
        ReturnErrorOnFailure(err);
        return operation.WaitForCompletion();
    }

    template <typename RequestT>
    CHIP_ERROR InvokeCommand(NodeId nodeId, EndpointId endpoint, const RequestT & request)
    {
        BlockingInvokeCommandOperation<RequestT> operation(nodeId, endpoint, request);
        return RunConnectionOperation(operation);
    }

    template <typename AttributeInfo>
    CHIP_ERROR WriteAttribute(NodeId nodeId, EndpointId endpoint, const typename AttributeInfo::Type & value)
    {
        BlockingWriteAttributeOperation<AttributeInfo> operation(nodeId, endpoint, value);
        return RunConnectionOperation(operation);
    }

    template <typename RequestT>
    CHIP_ERROR InvokeGroupCommand(GroupId groupId, const RequestT & request)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, groupId, &request, &err]() {
            auto * exchangeMgr = chip::app::InteractionModelEngine::GetInstance()->GetExchangeManager();
            if (exchangeMgr == nullptr)
            {
                err = CHIP_ERROR_INCORRECT_STATE;
                return;
            }

            err = chip::Controller::InvokeGroupCommandRequest(exchangeMgr, mCommissioner->GetFabricIndex(), groupId, request);
        }));
        return err;
    }

    CHIP_ERROR RemoveGroupKey(FabricIndex fabric, GroupId groupId)
    {
        auto * iter = mGroupDataProvider.IterateGroupKeys(fabric);
        VerifyOrReturnError(iter != nullptr, CHIP_ERROR_INCORRECT_STATE);

        CHIP_ERROR status = CHIP_ERROR_NOT_FOUND;
        size_t index      = 0;
        chip::Credentials::GroupDataProvider::GroupKey groupKey;
        while (iter->Next(groupKey))
        {
            if (groupKey.group_id == groupId)
            {
                status = mGroupDataProvider.RemoveGroupKeyAt(fabric, index);
                break;
            }
            index++;
        }
        iter->Release();
        return status;
    }

    CHIP_ERROR LoadOrCreateRhythmGroupEpochKey(RhythmGroupEpochKey & epochKey)
    {
        VerifyOrReturnError(mStorage != nullptr, CHIP_ERROR_INCORRECT_STATE);

        uint16_t size = static_cast<uint16_t>(epochKey.size());
        CHIP_ERROR err = mStorage->SyncGetKeyValue(kRhythmGroupEpochKeyStorageKey, epochKey.data(), size);
        if (err == CHIP_NO_ERROR && size == epochKey.size())
        {
            ChipLogProgress(Controller, "Rhythm Matter group epoch key loaded: storage_key=%s bytes=%u",
                            kRhythmGroupEpochKeyStorageKey, static_cast<unsigned>(epochKey.size()));
            return CHIP_NO_ERROR;
        }

        if (err != CHIP_NO_ERROR && err != CHIP_ERROR_PERSISTED_STORAGE_VALUE_NOT_FOUND &&
            err != CHIP_ERROR_BUFFER_TOO_SMALL)
        {
            return err;
        }

        ReturnErrorOnFailure(chip::Crypto::DRBG_get_bytes(epochKey.data(), epochKey.size()));
        ReturnErrorOnFailure(mStorage->SyncSetKeyValue(kRhythmGroupEpochKeyStorageKey, epochKey.data(),
                                                       static_cast<uint16_t>(epochKey.size())));
        ChipLogProgress(Controller, "Rhythm Matter group epoch key created: storage_key=%s bytes=%u",
                        kRhythmGroupEpochKeyStorageKey, static_cast<unsigned>(epochKey.size()));
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR EnsureControllerRhythmGroupKeySet(FabricIndex fabric)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        RhythmGroupEpochKey epochKey;
        ReturnErrorOnFailure(LoadOrCreateRhythmGroupEpochKey(epochKey));

        CHIP_ERROR providerStatus = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, fabric, &epochKey, &providerStatus]() {
            uint8_t compressedFabricId[sizeof(uint64_t)] = { 0 };
            chip::MutableByteSpan compressedFabricIdSpan(compressedFabricId);
            providerStatus = mCommissioner->GetCompressedFabricIdBytes(compressedFabricIdSpan);
            if (providerStatus != CHIP_NO_ERROR)
            {
                return;
            }

            chip::Credentials::GroupDataProvider::KeySet keySet(
                kRhythmGroupKeySetId, chip::Credentials::GroupDataProvider::SecurityPolicy::kTrustFirst, 1);
            keySet.epoch_keys[0].start_time = kRhythmGroupEpochStartTime;
            std::memcpy(keySet.epoch_keys[0].key, epochKey.data(), epochKey.size());
            providerStatus = mGroupDataProvider.SetKeySet(fabric, compressedFabricIdSpan, keySet);
        }));
        ReturnErrorOnFailure(providerStatus);

        ChipLogProgress(Controller, "Rhythm Matter group controller keyset ready: keyset=%u fabric=%u epoch_start=%llu",
                        static_cast<unsigned>(kRhythmGroupKeySetId), static_cast<unsigned>(fabric),
                        static_cast<unsigned long long>(kRhythmGroupEpochStartTime));
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR ReadGroupKeyMap(NodeId nodeId,
                               std::vector<GroupKeyManagement::Structs::GroupKeyMapStruct::Type> & out)
    {
        CHIP_ERROR iterErr = CHIP_NO_ERROR;
        CHIP_ERROR err = ReadAttribute<GroupKeyManagement::Attributes::GroupKeyMap::TypeInfo>(
            nodeId, kRootEndpoint, [&out, &iterErr](const auto & value) {
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

    CHIP_ERROR ProvisionGroupKeyOnDevice(NodeId nodeId, GroupId groupId)
    {
        VerifyOrReturnError(mCommissioner != nullptr, CHIP_ERROR_INCORRECT_STATE);

        const FabricIndex fabric = mCommissioner->GetFabricIndex();
        RhythmGroupEpochKey epochKey;
        ReturnErrorOnFailure(LoadOrCreateRhythmGroupEpochKey(epochKey));

        GroupKeyManagement::Commands::KeySetWrite::Type keySetWrite;
        keySetWrite.groupKeySet.groupKeySetID          = kRhythmGroupKeySetId;
        keySetWrite.groupKeySet.groupKeySecurityPolicy = GroupKeyManagement::GroupKeySecurityPolicyEnum::kTrustFirst;
        keySetWrite.groupKeySet.epochKey0.SetNonNull(chip::ByteSpan(epochKey.data(), epochKey.size()));
        keySetWrite.groupKeySet.epochStartTime0.SetNonNull(kRhythmGroupEpochStartTime);
        keySetWrite.groupKeySet.epochKey1.SetNull();
        keySetWrite.groupKeySet.epochStartTime1.SetNull();
        keySetWrite.groupKeySet.epochKey2.SetNull();
        keySetWrite.groupKeySet.epochStartTime2.SetNull();
        keySetWrite.groupKeySet.groupKeyMulticastPolicy = GroupKeyManagement::GroupKeyMulticastPolicyEnum::kPerGroupID;

        CHIP_ERROR keySetStatus = InvokeCommand(nodeId, kRootEndpoint, keySetWrite);
        if (keySetStatus != CHIP_NO_ERROR)
        {
            ChipLogError(Controller,
                         "Rhythm Matter group keyset write failed: group=%u node=" ChipLogFormatX64
                         " keyset=%u error=%" CHIP_ERROR_FORMAT,
                         static_cast<unsigned>(groupId), ChipLogValueX64(nodeId),
                         static_cast<unsigned>(kRhythmGroupKeySetId), keySetStatus.Format());
            return keySetStatus;
        }
        ChipLogProgress(Controller,
                        "Rhythm Matter group keyset written: group=%u node=" ChipLogFormatX64
                        " keyset=%u endpoint=%u epoch_start=%llu",
                        static_cast<unsigned>(groupId), ChipLogValueX64(nodeId),
                        static_cast<unsigned>(kRhythmGroupKeySetId), static_cast<unsigned>(kRootEndpoint),
                        static_cast<unsigned long long>(kRhythmGroupEpochStartTime));

        std::vector<GroupKeyManagement::Structs::GroupKeyMapStruct::Type> currentMap;
        CHIP_ERROR mapReadStatus = ReadGroupKeyMap(nodeId, currentMap);
        if (mapReadStatus != CHIP_NO_ERROR)
        {
            ChipLogError(Controller,
                         "Rhythm Matter group key map read failed: group=%u node=" ChipLogFormatX64
                         " keyset=%u error=%" CHIP_ERROR_FORMAT,
                         static_cast<unsigned>(groupId), ChipLogValueX64(nodeId),
                         static_cast<unsigned>(kRhythmGroupKeySetId), mapReadStatus.Format());
            return mapReadStatus;
        }

        std::vector<GroupKeyManagement::Structs::GroupKeyMapStruct::Type> updatedMap;
        updatedMap.reserve(currentMap.size() + 1);
        bool mapped = false;
        for (auto entry : currentMap)
        {
            if (entry.groupId == groupId)
            {
                if (!mapped)
                {
                    entry.groupKeySetID = kRhythmGroupKeySetId;
                    entry.fabricIndex   = fabric;
                    updatedMap.push_back(entry);
                    mapped = true;
                }
                continue;
            }
            updatedMap.push_back(entry);
        }

        if (!mapped)
        {
            GroupKeyManagement::Structs::GroupKeyMapStruct::Type mapping;
            mapping.groupId       = groupId;
            mapping.groupKeySetID = kRhythmGroupKeySetId;
            mapping.fabricIndex   = fabric;
            updatedMap.push_back(mapping);
        }

        GroupKeyManagement::Attributes::GroupKeyMap::TypeInfo::Type mapWrite(updatedMap.data(), updatedMap.size());
        CHIP_ERROR mapWriteStatus =
            WriteAttribute<GroupKeyManagement::Attributes::GroupKeyMap::TypeInfo>(nodeId, kRootEndpoint, mapWrite);
        if (mapWriteStatus != CHIP_NO_ERROR)
        {
            ChipLogError(Controller,
                         "Rhythm Matter group key map write failed: group=%u node=" ChipLogFormatX64
                         " keyset=%u entries=%zu error=%" CHIP_ERROR_FORMAT,
                         static_cast<unsigned>(groupId), ChipLogValueX64(nodeId),
                         static_cast<unsigned>(kRhythmGroupKeySetId), updatedMap.size(), mapWriteStatus.Format());
            return mapWriteStatus;
        }

        ChipLogProgress(Controller,
                        "Rhythm Matter group key map written: group=%u node=" ChipLogFormatX64
                        " keyset=%u entries=%zu fabric=%u",
                        static_cast<unsigned>(groupId), ChipLogValueX64(nodeId),
                        static_cast<unsigned>(kRhythmGroupKeySetId), updatedMap.size(), static_cast<unsigned>(fabric));
        return CHIP_NO_ERROR;
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

    template <typename AttributeInfo>
    CHIP_ERROR ReadCommandListAttribute(NodeId nodeId, EndpointId endpoint, std::vector<CommandId> & out)
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
    CHIP_ERROR ReadAttributeIdListAttribute(NodeId nodeId, EndpointId endpoint, std::vector<AttributeId> & out)
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

    CHIP_ERROR ReadDeviceTypeListAttribute(
        NodeId nodeId, EndpointId endpoint,
        std::vector<Descriptor::Structs::DeviceTypeStruct::DecodableType> & out)
    {
        CHIP_ERROR iterErr = CHIP_NO_ERROR;
        CHIP_ERROR err = ReadAttribute<Descriptor::Attributes::DeviceTypeList::TypeInfo>(
            nodeId, endpoint, [&out, &iterErr](const auto & value) {
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

    // The three helpers below must run on the Matter thread: they read and
    // destroy ReadClient-owning operations.
    bool HasLightStateSubscription(NodeId nodeId, EndpointId endpoint) const
    {
        return std::any_of(mLightStateSubscriptions.begin(), mLightStateSubscriptions.end(),
                           [nodeId, endpoint](const LightStateSubscriptionEntry & entry) {
                               return entry.nodeId == nodeId && entry.endpoint == endpoint && entry.operation != nullptr &&
                                   entry.operation->IsActive();
                           });
    }

    void PruneInactiveLightStateSubscriptions()
    {
        mLightStateSubscriptions.erase(
            std::remove_if(mLightStateSubscriptions.begin(), mLightStateSubscriptions.end(),
                           [](const LightStateSubscriptionEntry & entry) {
                               return entry.operation == nullptr || !entry.operation->IsActive();
                           }),
            mLightStateSubscriptions.end());
    }

    void RemoveLightStateSubscriptionsForNode(NodeId nodeId)
    {
        mLightStateSubscriptions.erase(
            std::remove_if(mLightStateSubscriptions.begin(), mLightStateSubscriptions.end(),
                           [nodeId](const LightStateSubscriptionEntry & entry) { return entry.nodeId == nodeId; }),
            mLightStateSubscriptions.end());
        // Tombstone the unpaired node so a SubscribeOnOff that is still in
        // flight for it cannot re-install a subscription afterwards. Cleared
        // when the node is commissioned again, and on controller init.
        mUnpairedNodes.insert(nodeId);
    }

    void QueueLightStateReport(NodeId nodeId, EndpointId endpoint, uint32_t clusterId, uint32_t attributeId,
                               uint8_t valueType, bool boolValue, uint64_t unsignedValue)
    {
        rhythm_chip_bridge_attribute_report report;
        report.node_id      = nodeId;
        report.endpoint     = endpoint;
        report.cluster_id   = clusterId;
        report.attribute_id = attributeId;
        report.value_type   = valueType;
        report.bool_value   = boolValue;
        report.unsigned_value = unsignedValue;
        report.received_at_unix_ms = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
                                                               std::chrono::system_clock::now().time_since_epoch())
                                                               .count());

        std::lock_guard<std::mutex> lock(mReportMutex);
        constexpr size_t kMaxQueuedAttributeReports = 1024;
        if (mAttributeReports.size() >= kMaxQueuedAttributeReports)
        {
            mAttributeReports.erase(mAttributeReports.begin());
        }
        mAttributeReports.push_back(report);
    }

    // Called on the Matter thread from LightStateSubscriptionOperation::OnDone once
    // an established subscription has ended. Only established subscriptions
    // reach here; pre-establishment failures surface as the SubscribeOnOff RPC
    // error instead.
    void QueueSubscriptionTermination(NodeId nodeId, EndpointId endpoint, CHIP_ERROR error)
    {
        rhythm_chip_bridge_subscription_termination termination;
        termination.node_id       = nodeId;
        termination.endpoint      = endpoint;
        termination.chip_error    = static_cast<uint32_t>(error.AsInteger());
        termination.failure_class = ClassifySubscriptionFailure(error);

        std::lock_guard<std::mutex> lock(mReportMutex);
        constexpr size_t kMaxQueuedSubscriptionTerminations = 256;
        if (mSubscriptionTerminations.size() >= kMaxQueuedSubscriptionTerminations)
        {
            mSubscriptionTerminations.erase(mSubscriptionTerminations.begin());
        }
        mSubscriptionTerminations.push_back(termination);
    }

    std::mutex mMutex;
    std::unique_ptr<PersistentStorage> mStorage;
    std::unique_ptr<chip::Credentials::FileAttestationTrustStore> mPaaTrustStore;
    chip::Credentials::GroupDataProviderImpl mGroupDataProvider;
    chip::Credentials::PersistentStorageOpCertStore mOpCertStore;
    RhythmOperationalCredentialsIssuer mOperationalCredentialsIssuer;
    chip::Crypto::RawKeySessionKeystore mSessionKeystore;
    chip::Crypto::P256Keypair mOperationalKeypair;
    BlockingPairingDelegate mPairingDelegate;
    std::unique_ptr<DeviceCommissioner> mCommissioner;
    // Matter-thread-owned subscription table plus the reservations and unpair
    // tombstones that keep concurrent RPCs from racing it.
    std::vector<LightStateSubscriptionEntry> mLightStateSubscriptions;
    std::set<std::pair<NodeId, EndpointId>> mPendingLightStateSubscriptions;
    std::set<NodeId> mUnpairedNodes;
    std::mutex mReportMutex;
    std::vector<rhythm_chip_bridge_attribute_report> mAttributeReports;
    std::vector<rhythm_chip_bridge_subscription_termination> mSubscriptionTerminations;
    std::string mStoragePath;
    std::string mFabricId;
    uint64_t mOperationalFabricId = 0;
    uint64_t mCompressedFabricId  = 0;
    RhythmIpk mIpk                = {};
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

bool WriteJsonOutput(const std::string & json, char * outJson, size_t jsonSize, size_t * outJsonLen,
                     char * errorMessage, size_t errorMessageSize)
{
    if (outJsonLen == nullptr)
    {
        WriteErrorMessage(errorMessage, errorMessageSize, "JSON read requires output length pointer");
        return false;
    }
    *outJsonLen = json.size();
    if (outJson == nullptr || jsonSize == 0)
    {
        WriteErrorMessage(errorMessage, errorMessageSize, "JSON read requires output buffer");
        return false;
    }
    if (json.size() >= jsonSize)
    {
        WriteErrorMessage(errorMessage, errorMessageSize, "JSON read output buffer too small");
        return false;
    }
    std::memcpy(outJson, json.data(), json.size());
    outJson[json.size()] = '\0';
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
#else
    return "connectedhomeip-unknown";
#endif
}

bool rhythm_chip_bridge_init(const char * storage_path, const char * fabric_id, uint64_t operational_fabric_id,
                             const char * ipk_hex, bool has_ble_controller, uint16_t ble_controller,
                             uint16_t controller_vendor_id, uint64_t * out_compressed_fabric_id, char * error_message,
                             size_t error_message_size)
{
    CHIP_ERROR err = gContext.Init(storage_path, fabric_id, operational_fabric_id, ipk_hex, has_ble_controller,
                                   ble_controller, controller_vendor_id);
    const bool success =
        HandleBridgeResult(err, error_message, error_message_size, "initializing CHIP controller bridge");
    if (success && out_compressed_fabric_id != nullptr)
    {
        *out_compressed_fabric_id = gContext.CompressedFabricId();
    }
    return success;
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

bool rhythm_chip_bridge_configure_group(const struct rhythm_chip_bridge_group * group, char * error_message,
                                        size_t error_message_size)
{
    if (group == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "configure_group requires group");
        return false;
    }

    return HandleBridgeResult(gContext.ConfigureGroup(*group), error_message, error_message_size,
                              "configuring Matter group");
}

bool rhythm_chip_bridge_remove_group(uint16_t group_id, const struct rhythm_chip_bridge_group_member * members,
                                     size_t member_count, char * error_message, size_t error_message_size)
{
    return HandleBridgeResult(gContext.RemoveGroup(group_id, members, member_count), error_message, error_message_size,
                              "removing Matter group");
}

bool rhythm_chip_bridge_set_group_on_off(uint16_t group_id, bool on, char * error_message, size_t error_message_size)
{
    return HandleBridgeResult(gContext.SetGroupOnOff(group_id, on), error_message, error_message_size,
                              "setting Matter group on/off");
}

bool rhythm_chip_bridge_identify_group(uint16_t group_id, uint16_t duration_secs, char * error_message,
                                       size_t error_message_size)
{
    return HandleBridgeResult(gContext.IdentifyGroup(group_id, duration_secs), error_message, error_message_size,
                              "identifying Matter group");
}

bool rhythm_chip_bridge_set_group_brightness(uint16_t group_id, uint8_t level, bool has_transition_ms,
                                             uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetGroupBrightness(group_id, level, transition), error_message, error_message_size,
                              "setting Matter group brightness");
}

bool rhythm_chip_bridge_set_group_color_temperature(uint16_t group_id, uint16_t kelvin, bool has_transition_ms,
                                                    uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetGroupColorTemperature(group_id, kelvin, transition), error_message,
                              error_message_size, "setting Matter group color temperature");
}

bool rhythm_chip_bridge_set_group_xy(uint16_t group_id, float x, float y, bool has_transition_ms, uint32_t transition_ms,
                                     char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetGroupXy(group_id, x, y, transition), error_message, error_message_size,
                              "setting Matter group xy color");
}

bool rhythm_chip_bridge_set_group_hue_saturation(uint16_t group_id, uint8_t hue, uint8_t saturation,
                                                 bool has_transition_ms, uint32_t transition_ms, char * error_message,
                                                 size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetGroupHueSaturation(group_id, hue, saturation, transition), error_message,
                              error_message_size, "setting Matter group hue/saturation color");
}

bool rhythm_chip_bridge_identify_light(uint64_t node_id, uint16_t endpoint, uint16_t duration_secs, char * error_message,
                                       size_t error_message_size)
{
    return HandleBridgeResult(gContext.IdentifyLight(node_id, endpoint, duration_secs), error_message, error_message_size,
                              "identifying Matter light");
}

bool rhythm_chip_bridge_set_brightness(uint64_t node_id, uint16_t endpoint, uint8_t level, bool has_transition_ms,
                                       uint32_t transition_ms, char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetBrightness(node_id, endpoint, level, transition), error_message, error_message_size,
                              "setting Matter brightness");
}

bool rhythm_chip_bridge_run_level_command(uint64_t node_id, uint16_t endpoint, uint8_t command, uint8_t level_or_step,
                                          uint8_t step_mode, bool has_transition_ms, uint32_t transition_ms,
                                          char * error_message, size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.RunLevelCommand(node_id, endpoint, command, level_or_step, step_mode, transition),
                              error_message, error_message_size, "running Matter level command");
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

bool rhythm_chip_bridge_set_hue_saturation(uint64_t node_id, uint16_t endpoint, uint8_t hue, uint8_t saturation,
                                           bool has_transition_ms, uint32_t transition_ms, char * error_message,
                                           size_t error_message_size)
{
    const std::optional<uint32_t> transition = has_transition_ms ? std::optional<uint32_t>(transition_ms) : std::nullopt;
    return HandleBridgeResult(gContext.SetHueSaturation(node_id, endpoint, hue, saturation, transition), error_message,
                              error_message_size, "setting Matter hue/saturation color");
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

bool rhythm_chip_bridge_read_light_capability_snapshot(uint64_t node_id, uint16_t endpoint, char * out_json,
                                                       size_t json_size, size_t * out_json_len, char * error_message,
                                                       size_t error_message_size)
{
    std::string json;
    CHIP_ERROR err = gContext.ReadLightCapabilitySnapshot(node_id, endpoint, json);
    if (err != CHIP_NO_ERROR)
    {
        return HandleBridgeResult(err, error_message, error_message_size, "reading Matter light capability snapshot");
    }
    return WriteJsonOutput(json, out_json, json_size, out_json_len, error_message, error_message_size);
}

bool rhythm_chip_bridge_read_light_state(uint64_t node_id, uint16_t endpoint, char * out_json, size_t json_size,
                                         size_t * out_json_len, char * error_message, size_t error_message_size)
{
    std::string json;
    CHIP_ERROR err = gContext.ReadLightStateJson(node_id, endpoint, json);
    if (err != CHIP_NO_ERROR)
    {
        return HandleBridgeResult(err, error_message, error_message_size, "reading Matter light state");
    }
    return WriteJsonOutput(json, out_json, json_size, out_json_len, error_message, error_message_size);
}

bool rhythm_chip_bridge_write_color_control_options(uint64_t node_id, uint16_t endpoint, bool execute_if_off,
                                                    char * error_message, size_t error_message_size)
{
    return HandleBridgeResult(gContext.WriteColorControlOptions(node_id, endpoint, execute_if_off), error_message,
                              error_message_size, "writing Matter ColorControl Options");
}

bool rhythm_chip_bridge_subscribe_on_off(const struct rhythm_chip_bridge_subscription_target * targets, size_t target_count,
                                         uint16_t min_interval_secs, uint16_t max_interval_secs, bool replace_existing,
                                         char * error_message, size_t error_message_size)
{
    return HandleBridgeResult(gContext.SubscribeOnOff(targets, target_count, min_interval_secs, max_interval_secs,
                                                       replace_existing),
                              error_message, error_message_size, "subscribing to Matter on/off attributes");
}

bool rhythm_chip_bridge_drain_attribute_reports(struct rhythm_chip_bridge_attribute_report * reports, size_t reports_capacity,
                                                size_t * out_report_count, char * error_message, size_t error_message_size)
{
    if (out_report_count == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "drain_attribute_reports requires output count");
        return false;
    }

    *out_report_count = gContext.DrainAttributeReports(reports, reports_capacity);
    return true;
}

bool rhythm_chip_bridge_drain_subscription_terminations(struct rhythm_chip_bridge_subscription_termination * terminations,
                                                        size_t terminations_capacity, size_t * out_termination_count,
                                                        char * error_message, size_t error_message_size)
{
    if (out_termination_count == nullptr)
    {
        WriteErrorMessage(error_message, error_message_size, "drain_subscription_terminations requires output count");
        return false;
    }

    *out_termination_count = gContext.DrainSubscriptionTerminations(terminations, terminations_capacity);
    return true;
}

void rhythm_chip_bridge_shutdown(void)
{
    gContext.Shutdown();
}
