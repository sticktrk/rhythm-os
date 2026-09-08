package lighting.rhythm.app

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.IntentSender
import android.os.Build
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.home.matter.Matter
import com.google.android.gms.home.matter.commissioning.CommissioningRequest
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private val channelName = "lighting.rhythm.app/phone_matter_commissioner"

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, channelName)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "isSupported" -> checkPhoneMatterSupport(result)
                    "commission" -> startPhoneMatterCommissioning(
                        call.arguments as? Map<*, *>,
                        result,
                    )
                    else -> result.notImplemented()
                }
            }
    }

    private fun isPhoneMatterSupported(): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1 &&
            packageManager.hasSystemFeature("android.hardware.bluetooth_le") &&
            GoogleApiAvailability.getInstance()
                .isGooglePlayServicesAvailable(this) == ConnectionResult.SUCCESS

    private fun checkPhoneMatterSupport(result: MethodChannel.Result) {
        if (!isPhoneMatterSupported()) {
            result.success(false)
            return
        }
        val commissioningClient = Matter.getCommissioningClient(this)
        ModuleInstall.getClient(this).areModulesAvailable(commissioningClient)
            .addOnSuccessListener { availability ->
                result.success(availability.areModulesAvailable())
            }
            .addOnFailureListener { result.success(false) }
    }

    private fun startPhoneMatterCommissioning(
        arguments: Map<*, *>?,
        result: MethodChannel.Result,
    ) {
        if (!isPhoneMatterSupported()) {
            result.error(
                "native_unavailable",
                "Phone Matter commissioning is unavailable on this Android device.",
                null,
            )
            return
        }
        val baseUrl = (arguments?.get("base_url") as? String)?.trim().orEmpty()
        val setupPayload =
            (arguments?.get("setup_payload") as? String)?.trim().orEmpty()
        val sessionId = (arguments?.get("session_id") as? String)?.trim().orEmpty()
        val authToken = (arguments?.get("auth_token") as? String)?.trim()
        if (baseUrl.isEmpty() || setupPayload.isEmpty() || sessionId.isEmpty()) {
            result.error("handoff", "The Matter commissioning request is incomplete.", null)
            return
        }
        val session = PhoneMatterCommissioningSession(
            baseUrl = baseUrl,
            authToken = authToken,
            originalSetupPayload = setupPayload,
            sessionId = sessionId,
            flutterResult = result,
        )
        if (!PhoneMatterCommissioningCoordinator.shared.begin(session)) {
            result.error(
                "platform_commissioning",
                "Another Matter commissioning request is already active.",
                null,
            )
            return
        }

        val request = CommissioningRequest.builder()
            .setOnboardingPayload(setupPayload)
            // A custom CommissioningService keeps the resulting fabric in
            // Rhythm instead of asking Google Home to own the accessory.
            .setCommissioningService(
                ComponentName(this, PhoneMatterCommissioningService::class.java),
            )
            .build()
        Matter.getCommissioningClient(this).commissionDevice(request)
            .addOnSuccessListener { sender ->
                if (!PhoneMatterCommissioningCoordinator.shared.isActive(session)) {
                    return@addOnSuccessListener
                }
                try {
                    startIntentSenderForResult(
                        sender,
                        session.activityRequestCode,
                        null,
                        0,
                        0,
                        0,
                    )
                } catch (_: IntentSender.SendIntentException) {
                    PhoneMatterCommissioningCoordinator.shared.finishError(
                        session, "platform_commissioning",
                        "Android could not open Matter setup. Please try again.",
                    )
                }
            }
            .addOnFailureListener {
                PhoneMatterCommissioningCoordinator.shared.finishError(
                    session, "native_unavailable",
                    "Android Matter setup is unavailable. Update Google Play services and try again.",
                )
            }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val session = PhoneMatterCommissioningCoordinator.shared.forActivityResult(requestCode)
            ?: return
        if (resultCode == Activity.RESULT_OK) {
            PhoneMatterCommissioningCoordinator.shared.finishSuccess(session)
        } else {
            PhoneMatterCommissioningCoordinator.shared.finishError(
                session, "cancelled",
                "Matter setup was cancelled before the Rhythm Box finished pairing.",
            )
        }
    }

}
