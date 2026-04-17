#include "chip_bridge.h"

#include <app/InteractionModelEngine.h>
#include <controller/CHIPDeviceController.h>
#include <controller/CHIPDeviceControllerFactory.h>
#include <controller/ExampleOperationalCredentialsIssuer.h>
#include <controller/ExamplePersistentStorage.h>
#include <credentials/GroupDataProviderImpl.h>
#include <credentials/PersistentStorageOpCertStore.h>
#include <credentials/attestation_verifier/DefaultDeviceAttestationVerifier.h>
#include <crypto/RawKeySessionKeystore.h>
#include <lib/core/ErrorStr.h>
#include <lib/support/CodeUtils.h>
#include <lib/support/ScopedMemoryBuffer.h>
#include <lib/support/TestGroupData.h>
#include <platform/CHIPDeviceLayer.h>
#include <platform/TestOnlyCommissionableDataProvider.h>

#include <condition_variable>
#include <cstring>
#include <filesystem>
#include <functional>
#include <memory>
#include <mutex>
#include <string>

using chip::Controller::DeviceCommissioner;
using chip::Controller::DeviceControllerFactory;
using chip::Controller::ExampleOperationalCredentialsIssuer;
using chip::Controller::FactoryInitParams;
using chip::Controller::SetupParams;

namespace {

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

std::string FormatChipError(CHIP_ERROR error, const std::string & context)
{
    if (error == CHIP_NO_ERROR)
    {
        return context;
    }

    return context + ": " + chip::ErrorStr(error);
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

class ChipBridgeContext
{
public:
    CHIP_ERROR Init(const char * storagePath, const char * fabricId, bool hasBleController, uint16_t bleController)
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
                return CHIP_NO_ERROR;
            }
            return CHIP_ERROR_INCORRECT_STATE;
        }

        mStoragePath = requestedStorage;
        mFabricId    = fabricId != nullptr ? fabricId : "";

        if (!mFactoryInitialized)
        {
            ReturnErrorOnFailure(InitializeFactory(hasBleController, bleController));
        }

        CHIP_ERROR err = CHIP_NO_ERROR;
        ReturnErrorOnFailure(ExecuteOnMatterThread([this, &err]() { err = InitializeCommissioner(); }));
        return err;
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
    }

private:
    CHIP_ERROR InitializeFactory(bool hasBleController, uint16_t bleController)
    {
        ReturnErrorOnFailure(chip::Platform::MemoryInit());

#if CHIP_DEVICE_LAYER_TARGET_LINUX && CHIP_DEVICE_CONFIG_ENABLE_CHIPOBLE
        if (hasBleController)
        {
            ReturnErrorOnFailure(
                chip::DeviceLayer::Internal::BLEMgrImpl().ConfigureBle(bleController, /* BLE central */ true));
        }
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
        return CHIP_NO_ERROR;
    }

    CHIP_ERROR InitializeCommissioner()
    {
        if (mCommissioner != nullptr)
        {
            return CHIP_NO_ERROR;
        }

        auto * dacVerifier = chip::Credentials::GetDefaultDACVerifier(
            chip::Credentials::GetTestAttestationTrustStore(),
            /* revocationDelegate = */ nullptr);
        VerifyOrReturnError(dacVerifier != nullptr, CHIP_ERROR_INCORRECT_STATE);
        chip::Credentials::SetDeviceAttestationVerifier(dacVerifier);

        auto commissioner = std::make_unique<DeviceCommissioner>();

        SetupParams commissionerParams;
        commissionerParams.deviceAttestationVerifier      = dacVerifier;
        commissionerParams.operationalCredentialsDelegate = &mOperationalCredentialsIssuer;

        chip::Crypto::P256Keypair ephemeralKey;
        ReturnErrorOnFailure(ephemeralKey.Initialize(chip::Crypto::ECPKeyTarget::ECDSA));
        commissionerParams.operationalKeypair = &ephemeralKey;

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
            mStorage->GetLocalNodeId(), /* fabricId = */ 1, mStorage->GetCommissionerCATs(), ephemeralKey.Pubkey(), rcacSpan,
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

    std::mutex mMutex;
    std::unique_ptr<PersistentStorage> mStorage;
    chip::Credentials::GroupDataProviderImpl mGroupDataProvider;
    chip::Credentials::PersistentStorageOpCertStore mOpCertStore;
    ExampleOperationalCredentialsIssuer mOperationalCredentialsIssuer;
    chip::Crypto::RawKeySessionKeystore mSessionKeystore;
    std::unique_ptr<DeviceCommissioner> mCommissioner;
    std::string mStoragePath;
    std::string mFabricId;
    bool mFactoryInitialized = false;
    bool mEventLoopStarted   = false;
};

ChipBridgeContext gContext;

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
                             uint16_t ble_controller, char * error_message, size_t error_message_size)
{
    const CHIP_ERROR err = gContext.Init(storage_path, fabric_id, has_ble_controller, ble_controller);
    if (err != CHIP_NO_ERROR)
    {
        WriteErrorMessage(error_message, error_message_size, FormatChipError(err, "initializing CHIP controller bridge"));
        return false;
    }

    WriteErrorMessage(error_message, error_message_size, "");
    return true;
}

void rhythm_chip_bridge_shutdown(void)
{
    gContext.Shutdown();
}
