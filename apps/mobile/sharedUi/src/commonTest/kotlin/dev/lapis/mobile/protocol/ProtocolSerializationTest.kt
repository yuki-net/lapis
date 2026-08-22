package dev.lapis.mobile.protocol

import kotlinx.serialization.encodeToString
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ProtocolSerializationTest {
    @Test
    fun testClientHelloAndAuthSerialization() {
        val hello = ClientHello(
            protocol = ProtocolRange.EXACT_V0,
            clientId = "test-client-1",
            clientName = "Test Mobile",
            clientKind = ClientKind.Android,
        )
        val authReq = AuthenticateRequest(
            hello = hello,
            workspaceId = "workspace-default",
            credentialId = "cred-1",
            secret = "secret-hex",
        )
        val msg: ClientMessage = ClientMessage.Authenticate(authReq)
        val jsonStr = lapisJson.encodeToString(msg)

        assertTrue(jsonStr.contains("auth.authenticate"))
        assertTrue(jsonStr.contains("workspace-default"))

        val decoded = lapisJson.decodeFromString<ClientMessage>(jsonStr)
        assertEquals(msg, decoded)
    }

    @Test
    fun testServerHelloSerialization() {
        val serverHello = ServerHello(
            protocol = ProtocolVersion(0, 0),
            sessionId = "session-123",
            grantedCapabilities = listOf("files.read", "terminal.start"),
        )
        val serverMsg: ServerMessage = ServerMessage.Authenticated(serverHello)
        val jsonStr = lapisJson.encodeToString(serverMsg)

        assertTrue(jsonStr.contains("auth.authenticated"))
        assertTrue(jsonStr.contains("session-123"))

        val decoded = lapisJson.decodeFromString<ServerMessage>(jsonStr)
        assertEquals(serverMsg, decoded)
    }
}
