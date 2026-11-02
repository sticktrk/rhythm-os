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
    fun haHandoffKeepsOriginalAndTemporaryMaterialSeparate() {
        val callback = object : MethodChannel.Result {
            override fun success(result: Any?) { fail("Unexpected callback") }
            override fun error(code: String, message: String?, details: Any?) { fail(code) }
            override fun notImplemented() { fail("Unexpected callback") }
        }
        for (source in listOf("original_label", "sharing")) {
            val session = PhoneMatterCommissioningSession(
                "http://192.0.2.1", "owner-token", "MT:OWNER", "ha-session", callback,
                "ha_addon", source)
            assertEquals("/api/addon/matter/pair", session.handoffPath)
            val body = phoneMatterHandoffBody(session, "192.0.2.44", 5540, 20202021)
            assertEquals("MT:OWNER", body.getString("setup_code"))
            assertEquals(source, body.getString("code_source"))
            assertEquals("192.0.2.44", body.getString("handoff_ip_address"))
            assertEquals(20202021, body.getLong("handoff_passcode"))
            assertFalse(body.has("params"))
        }
        assertThrows(IllegalArgumentException::class.java) {
            PhoneMatterCommissioningSession("http://192.0.2.1", null, "MT:OWNER", "session", callback,
                "https://external.invalid", "original_label")
        }
    }

    @Test
    fun nativeCompletionWaitsForMatchingConfirmedHaReceipt() {
        fun receipt(id: String, status: String) = mapOf<String, Any?>(
            "http_status" to 200, "body" to mapOf("session_id" to id, "status" to status))
        var polls = 0
        val result = awaitHaMatterCompletion("attempt", receipt("attempt", "pending"),
            poll = { polls++; receipt("attempt", "completed") }, wait = {})
        assertEquals(1, polls)
        assertEquals("completed", (result["body"] as Map<*, *>)["status"])
        for (status in listOf("unknown", "failed", "future")) {
            assertThrows(ServerPairingException::class.java) {
                awaitHaMatterCompletion("attempt", receipt("attempt", status),
                    poll = { fail("Must not replay an unknown result"); result }, wait = {})
            }
        }
        assertThrows(IllegalArgumentException::class.java) {
            awaitHaMatterCompletion("attempt", receipt("other", "completed"), poll = { result })
        }
        assertThrows(InterruptedException::class.java) {
            awaitHaMatterCompletion("attempt", receipt("attempt", "pending"), poll = { result },
                isActive = { false }, wait = {})
        }
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
