package lighting.rhythm.app

import android.app.Service
import android.content.Intent
import android.os.IBinder
import com.google.android.gms.home.matter.commissioning.CommissioningCompleteMetadata
import com.google.android.gms.home.matter.commissioning.CommissioningRequestMetadata
import com.google.android.gms.home.matter.commissioning.CommissioningService
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject

class PhoneMatterCommissioningService : Service(), CommissioningService.Callback {
    private val executor = Executors.newSingleThreadExecutor()
    private val delegate by lazy {
        CommissioningService.Builder(this)
            .setCallback(this)
            .build()
    }

    override fun onBind(intent: Intent?): IBinder = delegate.asBinder()

    override fun onDestroy() {
        executor.shutdownNow()
        super.onDestroy()
    }

    override fun onCommissioningRequested(metadata: CommissioningRequestMetadata) {
        val session = PhoneMatterCommissioningCoordinator.shared.snapshot()
        if (session == null) {
            delegate.sendCommissioningError(CommissioningService.CommissioningError.OTHER)
            return
        }
        executor.execute {
            try {
                val response = postHandoff(
                    session = session,
                    address = metadata.networkLocation.formattedIpAddress,
                    port = metadata.networkLocation.port,
                    passcode = metadata.passcode,
                )
                if (!PhoneMatterCommissioningCoordinator.shared.recordServerResponse(session, response)) {
                    return@execute
                }
                delegate.sendCommissioningComplete(
                    CommissioningCompleteMetadata.builder()
                        .setToken(session.sessionId)
                        .build(),
                ).addOnFailureListener {
                    PhoneMatterCommissioningCoordinator.shared.finishError(
                        session, "handoff",
                        "Android could not finish the Matter handoff. Please try again.",
                    )
                }
            } catch (error: ServerPairingException) {
                if (PhoneMatterCommissioningCoordinator.shared.finishError(
                    session, "server_rejected",
                    error.safeMessage,
                )) {
                    delegate.sendCommissioningError(CommissioningService.CommissioningError.OTHER)
                }
            } catch (_: Exception) {
                if (PhoneMatterCommissioningCoordinator.shared.finishError(
                    session, "handoff",
                    "The phone could not reach the Rhythm Box to finish pairing.",
                )) {
                    delegate.sendCommissioningError(CommissioningService.CommissioningError.OTHER)
                }
            }
        }
    }

    private fun postHandoff(
        session: PhoneMatterCommissioningSession,
        address: String,
        port: Int,
        passcode: Long,
    ): Map<String, Any?> {
        val endpoint = session.baseUrl.trimEnd('/') + "/api/devices/pair"
        val connection = URL(endpoint).openConnection() as HttpURLConnection
        try {
            if (!PhoneMatterCommissioningCoordinator.shared.attachHandoff(session, connection)) {
                throw InterruptedException("Commissioning attempt ended")
            }
            connection.requestMethod = "POST"
            connection.connectTimeout = 15_000
            connection.readTimeout = 240_000
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json")
            session.authToken?.takeIf { it.isNotEmpty() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            val body = phoneMatterHandoffBody(session, address, port, passcode)
            connection.outputStream.use { output ->
                output.write(body.toString().toByteArray(Charsets.UTF_8))
            }
            val status = connection.responseCode
            val stream = if (status in 200..299) {
                connection.inputStream
            } else {
                connection.errorStream
            }
            val responseText = stream?.bufferedReader()?.use { it.readText() }.orEmpty()
            return phoneMatterHandoffResponse(status, responseText)
        } finally {
            PhoneMatterCommissioningCoordinator.shared.detachHandoff(session, connection)
            connection.disconnect()
        }
    }
}

internal class ServerPairingException(val safeMessage: String) : Exception()

internal fun phoneMatterHandoffBody(
    session: PhoneMatterCommissioningSession, address: String, port: Int, passcode: Long,
): JSONObject = JSONObject()
    .put("hub_type", "matter")
    .put("session_id", session.sessionId)
    .put("params", JSONObject()
        .put("setup_payload", session.originalSetupPayload)
        .put("network", "wifi")
        .put("rendezvous", "phone")
        .put("session_id", session.sessionId)
        .put("handoff_address", address)
        .put("handoff_port", port)
        .put("handoff_passcode", passcode))

// Only HTTP rejection belongs to the transport. The shared Dart response
// parser owns every 200 receipt, including failed/pending/future statuses.
internal fun phoneMatterHandoffResponse(status: Int, text: String): Map<String, Any?> {
    val body = runCatching { JSONObject(text) }.getOrNull()
    if (status != HttpURLConnection.HTTP_OK) {
        val message = body?.optString("error").orEmpty()
            .ifEmpty { body?.optString("message").orEmpty() }
            .take(240)
            .ifEmpty { "The Rhythm Box could not finish Matter pairing." }
        throw ServerPairingException(message)
    }
    requireNotNull(body) { "Invalid Matter receipt" }
    return mapOf("http_status" to status, "body" to jsonObjectToMap(body))
}

private fun jsonObjectToMap(value: JSONObject): Map<String, Any?> =
    value.keys().asSequence().associateWith { key -> jsonValue(value.get(key)) }

private fun jsonValue(value: Any?): Any? = when (value) {
    JSONObject.NULL -> null
    is JSONObject -> jsonObjectToMap(value)
    is JSONArray -> (0 until value.length()).map { jsonValue(value.get(it)) }
    else -> value
}
