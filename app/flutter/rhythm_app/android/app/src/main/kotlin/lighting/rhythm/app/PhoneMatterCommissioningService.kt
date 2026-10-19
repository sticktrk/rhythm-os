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
        val response = requestHandoff(session, session.handoffPath, "POST",
            phoneMatterHandoffBody(session, address, port, passcode))
        if (session.backend != "ha_addon") return response
        return awaitHaMatterCompletion(session.sessionId, response,
            poll = { requestHandoff(session, "/api/addon/matter/pairing/${session.sessionId}", "GET", null) },
            isActive = { PhoneMatterCommissioningCoordinator.shared.isActive(session) })
    }

    private fun requestHandoff(
        session: PhoneMatterCommissioningSession, path: String, method: String, body: JSONObject?,
    ): Map<String, Any?> {
        val endpoint = session.baseUrl.trimEnd('/') + path
        val connection = URL(endpoint).openConnection() as HttpURLConnection
        try {
            if (!PhoneMatterCommissioningCoordinator.shared.attachHandoff(session, connection)) {
                throw InterruptedException("Commissioning attempt ended")
            }
            connection.requestMethod = method
            connection.connectTimeout = 15_000
            connection.readTimeout = if (method == "GET") 10_000 else 240_000
            connection.doOutput = body != null
            connection.setRequestProperty("Content-Type", "application/json")
            session.authToken?.takeIf { it.isNotEmpty() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            if (body != null) {
                connection.outputStream.use { output ->
                    output.write(body.toString().toByteArray(Charsets.UTF_8))
                }
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

/** Only GET receipts repeat while Android keeps the commissioning window open. */
internal fun awaitHaMatterCompletion(
    sessionId: String,
    initial: Map<String, Any?>,
    poll: () -> Map<String, Any?>,
    isActive: () -> Boolean = { true },
    wait: () -> Unit = { Thread.sleep(1_500) },
): Map<String, Any?> {
    val deadline = System.nanoTime() + 230_000_000_000L
    var response = initial
    while (true) {
        if (!isActive()) throw InterruptedException("Commissioning attempt ended")
        val body = response["body"] as? Map<*, *>
        require(body?.get("session_id") == sessionId) { "Mismatched commissioning receipt" }
        if (body?.get("status") == "completed") return response
        if (body?.get("status") != "pending" || System.nanoTime() >= deadline) {
            throw ServerPairingException("Home Assistant did not confirm pairing. Check this attempt in Rhythm before trying again.")
        }
        wait()
        response = poll()
    }
}

internal class ServerPairingException(val safeMessage: String) : Exception()

internal fun phoneMatterHandoffBody(
    session: PhoneMatterCommissioningSession, address: String, port: Int, passcode: Long,
): JSONObject = if (session.backend == "ha_addon") JSONObject()
    .put("session_id", session.sessionId)
    .put("setup_code", session.originalSetupPayload)
    .put("code_source", session.codeSource)
    .put("rendezvous", "phone")
    .put("handoff_ip_address", address)
    .put("handoff_port", port)
    .put("handoff_passcode", passcode)
else JSONObject()
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
