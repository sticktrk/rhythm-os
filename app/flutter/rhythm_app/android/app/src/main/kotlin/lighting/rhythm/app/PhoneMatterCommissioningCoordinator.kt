package lighting.rhythm.app

import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.MethodChannel
import java.net.HttpURLConnection

class PhoneMatterCommissioningSession(
    val baseUrl: String,
    val authToken: String?,
    val originalSetupPayload: String,
    val sessionId: String,
    val flutterResult: MethodChannel.Result,
) {
    internal var activityRequestCode: Int = -1
    internal var serverResponse: Map<String, Any?>? = null
    internal var connection: HttpURLConnection? = null
}

/** Each callback must retain its originating session, even after cancellation. */
class PhoneMatterCommissioningCoordinator(
    private val dispatch: (() -> Unit) -> Unit,
) {
    private var active: PhoneMatterCommissioningSession? = null
    private var nextActivityRequestCode = 9074

    @Synchronized
    fun begin(session: PhoneMatterCommissioningSession): Boolean {
        // Android activity request codes use the low 16 bits. Never reuse a
        // code in this process: an old Activity result must not complete a retry.
        if (active != null || nextActivityRequestCode > 0xffff) return false
        session.activityRequestCode = nextActivityRequestCode++
        active = session
        return true
    }

    @Synchronized
    fun snapshot(): PhoneMatterCommissioningSession? = active

    @Synchronized
    fun isActive(session: PhoneMatterCommissioningSession): Boolean = active === session

    @Synchronized
    fun forActivityResult(requestCode: Int): PhoneMatterCommissioningSession? =
        active?.takeIf { it.activityRequestCode == requestCode }

    @Synchronized
    fun attachHandoff(session: PhoneMatterCommissioningSession, connection: HttpURLConnection): Boolean {
        if (active !== session) return false
        session.connection = connection
        return true
    }

    @Synchronized
    fun detachHandoff(session: PhoneMatterCommissioningSession, connection: HttpURLConnection) {
        if (session.connection === connection) session.connection = null
    }

    @Synchronized
    fun recordServerResponse(session: PhoneMatterCommissioningSession, response: Map<String, Any?>): Boolean {
        if (active !== session) return false
        session.serverResponse = response
        return true
    }

    fun finishSuccess(session: PhoneMatterCommissioningSession): Boolean {
        val response = synchronized(this) {
            if (active !== session) return false
            active = null
            session.serverResponse
        }
        dispatch {
            if (response == null) {
                session.flutterResult.error(
                    "invalid_response",
                    "Android completed Matter setup without a Rhythm Box result.",
                    null,
                )
            } else {
                session.flutterResult.success(response)
            }
        }
        return true
    }

    fun finishError(session: PhoneMatterCommissioningSession, stage: String, message: String): Boolean {
        val connection = synchronized(this) {
            if (active !== session) return false
            active = null
            session.connection.also { session.connection = null }
        }
        // Fencing above remains authoritative if disconnect races with a response.
        runCatching { connection?.disconnect() }
        dispatch { session.flutterResult.error(stage, message, null) }
        return true
    }

    companion object {
        val shared = PhoneMatterCommissioningCoordinator { action ->
            Handler(Looper.getMainLooper()).post { action() }
        }
    }
}
