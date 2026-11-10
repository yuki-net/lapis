package dev.lapis.mobile.protocol

import kotlinx.serialization.encodeToString
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Rust `contracts/protocol/tests/wire_fixtures.rs` と同じ JSON を検証する。
 * wire 形式の正本は Rust 側の fixture。
 */
class ProtocolSerializationTest {
    @Test
    fun clientHelloMatchesRustFixture() {
        val json =
            """{"protocol":{"major":0,"minimum_minor":10,"maximum_minor":10},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]}"""

        val hello = lapisJson.decodeFromString<ClientHello>(json)

        assertEquals(ProtocolRange(0, 10, 10), hello.protocol)
        assertEquals(json, lapisJson.encodeToString(hello))
    }

    @Test
    fun authenticateMessageMatchesRustFixture() {
        val json =
            """{"type":"auth.authenticate","payload":{"hello":{"protocol":{"major":0,"minimum_minor":10,"maximum_minor":10},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]},"workspace_id":"workspace-1","credential_id":"0123456789abcdef0123456789abcdef","secret":"abababababababababababababababababababababababababababababababab"}}"""

        val message = lapisJson.decodeFromString<ClientMessage>(json)

        assertEquals(json, lapisJson.encodeToString(message))
    }

    @Test
    fun oidcMessageMatchesRustFixture() {
        val json =
            """{"type":"auth.oidc","payload":{"hello":{"protocol":{"major":0,"minimum_minor":10,"maximum_minor":10},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]},"workspace_id":"workspace-1","id_token":"test-token"}}"""

        val message = lapisJson.decodeFromString<ClientMessage>(json)

        assertEquals(json, lapisJson.encodeToString(message))
    }

    @Test
    fun authRequiredMatchesRustFixture() {
        val json =
            """{"type":"auth.required","payload":{"methods":["token","oidc"],"oidc":{"issuer":"https://idp.example","client_id":"lapis","scopes":["openid","profile"]}}}"""

        val message = lapisJson.decodeFromString<ServerMessage>(json)
        val encoded = lapisJson.encodeToString<ServerMessage>(message)

        val required = message as ServerMessage.AuthRequired
        assertEquals(listOf(AuthMethod.Token, AuthMethod.Oidc), required.payload.methods)
        assertEquals(
            OidcClientConfig(
                issuer = "https://idp.example",
                clientId = "lapis",
                scopes = listOf("openid", "profile"),
            ),
            required.payload.oidc,
        )
        assertEquals(json, encoded)
    }

    @Test
    fun requestEnvelopeMatchesRustFixture() {
        val json = """{"request_id":"request-1","type":"workspace.list","payload":{}}"""

        val envelope = lapisJson.decodeFromString<RequestEnvelope>(json)

        assertEquals("workspace.list", envelope.type)
        assertEquals(json, lapisJson.encodeToString(envelope))
    }

    @Test
    fun eventEnvelopeMatchesRustFixture() {
        val json =
            """{"event_sequence":7,"type":"workspace.changed","payload":{"workspace_id":"workspace-1"}}"""

        val envelope = lapisJson.decodeFromString<EventEnvelope>(json)

        assertEquals(7L, envelope.eventSequence)
        assertEquals("workspace.changed", envelope.type)
        assertEquals(json, lapisJson.encodeToString(envelope))
    }

    @Test
    fun protocolErrorMatchesRustFixture() {
        val json = """{"type":"error","payload":{"code":"invalid_request","retryable":false}}"""

        val message = lapisJson.decodeFromString<ServerMessage>(json)

        assertEquals(json, lapisJson.encodeToString(message))
    }

    @Test
    fun pairedResponseMatchesRustFixture() {
        val json =
            """{"type":"auth.paired","payload":{"hello":{"protocol":{"major":0,"minor":10},"session_id":"session-1","granted_capabilities":[]},"credential_id":"0123456789abcdef0123456789abcdef","secret":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}"""

        val message = lapisJson.decodeFromString<ServerMessage>(json)

        assertEquals(json, lapisJson.encodeToString(message))
    }
}
