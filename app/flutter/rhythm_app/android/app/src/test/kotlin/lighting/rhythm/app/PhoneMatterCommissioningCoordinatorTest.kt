package lighting.rhythm.app

import io.flutter.plugin.common.MethodChannel
import java.net.HttpURLConnection
import java.net.URL
import org.junit.Assert.*
import org.junit.Test

class PhoneMatterCommissioningCoordinatorTest {
    private class Receipt : MethodChannel.Result {
        val successes = mutableListOf<Any?>()
        val errors = mutableListOf<String>()
        override fun success(result: Any?) { successes.add(result) }
        override fun error(code: String, message: String?, details: Any?) { errors.add(code) }
        override fun notImplemented() { error("unexpected", null, null) }
    }

    private fun session(id: String, receipt: Receipt) = PhoneMatterCommissioningSession(
        "http://192.0.2.1:8080", null, "original-$id", id, receipt,
    )

    @Test
    fun cancelledAttemptCannotOverwriteOrFinishTheNextAttempt() {
        val coordinator = PhoneMatterCommissioningCoordinator { it() }
        val firstReceipt = Receipt()
        val nextReceipt = Receipt()
        val first = session("a", firstReceipt)
        val next = session("b", nextReceipt)
        assertTrue(coordinator.begin(first))
        assertTrue(coordinator.finishError(first, "cancelled", "Cancelled"))
        assertTrue(coordinator.begin(next))

        assertFalse(coordinator.recordServerResponse(first, mapOf("device_id" to "old-device")))
        assertFalse(coordinator.finishSuccess(first))
        assertFalse(coordinator.finishError(first, "handoff", "Late error"))
        assertSame(next, coordinator.snapshot())
        assertTrue(nextReceipt.errors.isEmpty())
        assertTrue(nextReceipt.successes.isEmpty())

        val response = mapOf("device_id" to "new-device")
        assertTrue(coordinator.recordServerResponse(next, response))
        assertTrue(coordinator.finishSuccess(next))
        assertEquals(listOf(response), nextReceipt.successes)
        assertEquals(listOf("cancelled"), firstReceipt.errors)
        assertFalse(coordinator.finishSuccess(next))
    }

    @Test
    fun lateActivityResultCannotSelectTheNextAttempt() {
        val coordinator = PhoneMatterCommissioningCoordinator { it() }
        val first = session("a", Receipt())
        val next = session("b", Receipt())
        assertTrue(coordinator.begin(first))
        assertFalse(coordinator.begin(next))
        coordinator.finishError(first, "cancelled", "Cancelled")
        assertTrue(coordinator.begin(next))
        assertNotEquals(first.activityRequestCode, next.activityRequestCode)
        assertNull(coordinator.forActivityResult(first.activityRequestCode))
        assertSame(next, coordinator.forActivityResult(next.activityRequestCode))
    }

    @Test
    fun cancellationDisconnectsOnlyItsOwnHandoffAndRejectsLateAttachment() {
        val coordinator = PhoneMatterCommissioningCoordinator { it() }
        val first = session("a", Receipt())
        val next = session("b", Receipt())
        var disconnected = false
        val connection = object : HttpURLConnection(URL("http://192.0.2.1")) {
            override fun connect() {}
            override fun usingProxy() = false
            override fun disconnect() { disconnected = true }
        }
        coordinator.begin(first)
        assertTrue(coordinator.attachHandoff(first, connection))
        coordinator.finishError(first, "cancelled", "Cancelled")
        assertTrue(disconnected)
        coordinator.begin(next)
        assertFalse(coordinator.attachHandoff(first, connection))
        assertSame(next, coordinator.snapshot())
    }

    @Test
    fun dispatchedCompletionKeepsItsOriginalRecipient() {
        val queue = mutableListOf<() -> Unit>()
        val coordinator = PhoneMatterCommissioningCoordinator { queue.add(it) }
        val firstReceipt = Receipt()
        val nextReceipt = Receipt()
        val first = session("a", firstReceipt)
        val next = session("b", nextReceipt)
        coordinator.begin(first)
        coordinator.finishError(first, "cancelled", "Cancelled")
        coordinator.begin(next)
        queue.single().invoke()
        assertEquals(listOf("cancelled"), firstReceipt.errors)
        assertTrue(nextReceipt.errors.isEmpty())
        assertSame(next, coordinator.snapshot())
    }
}
