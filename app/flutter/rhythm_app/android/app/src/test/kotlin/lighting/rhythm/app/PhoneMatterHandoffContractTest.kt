package lighting.rhythm.app

import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class PhoneMatterHandoffContractTest {
    private val fixture = JSONObject(javaClass.getResource("/phone_matter_contract.json")!!.readText())

    @Test
    fun realNativeEncoderMatchesServerFixture() {
        val session = PhoneMatterCommissioningSession(
            "http://192.0.2.1", null, "MT:ORIGINAL-OWNER-CODE", "phone-attempt-1",
            object : MethodChannel.Result {
                override fun success(result: Any?) { fail("Unexpected callback") }
                override fun error(code: String, message: String?, details: Any?) { fail(code) }
                override fun notImplemented() { fail("Unexpected callback") }
            },
        )
        assertEquals(fixture.getJSONObject("android_request").toString(),
            phoneMatterHandoffBody(session, "fe80::1234%phone-wifi", 5540, 20202021).toString())
    }

    @Test
    fun every200ReceiptPreservesWarningsRecoveryAndAdditiveFields() {
        for (status in listOf("failed", "complete", "pending", "future_status")) {
            val body = fixture.getJSONObject("failed_receipt").put("status", status)
            val response = phoneMatterHandoffResponse(200, body.toString())
            assertEquals(200, response["http_status"])
            val decoded = response["body"] as Map<*, *>
            assertEquals(body.keys().asSequence().toSet(), decoded.keys)
            assertEquals(status, decoded["status"])
            assertEquals(body.getString("error"), decoded["error"])
            assertNull(decoded["device"])
            assertEquals(mapOf("recovery_action" to "existing_node_recommission_failed"), decoded["details"])
            assertEquals(listOf("The original setup code remains available."), decoded["warnings"])
            assertEquals(mapOf("retained" to true), decoded["future_field"])
        }
    }

    @Test
    fun non200AndMalformedBodiesRemainTransportFailures() {
        val body = fixture.getJSONObject("failed_receipt")
        val error = assertThrows(ServerPairingException::class.java) {
            phoneMatterHandoffResponse(403, body.toString())
        }
        assertEquals(body.getString("error"), error.safeMessage)
        for (invalid in listOf("not json", "[]")) {
            assertThrows(IllegalArgumentException::class.java) {
                phoneMatterHandoffResponse(200, invalid)
            }
        }
    }
}
