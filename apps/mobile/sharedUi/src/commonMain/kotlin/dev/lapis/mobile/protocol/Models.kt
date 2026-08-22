package dev.lapis.mobile.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement

val lapisJson = Json {
    ignoreUnknownKeys = true
    isLenient = true
    encodeDefaults = true
}

@Serializable
enum class ClientKind {
    @SerialName("desktop") Desktop,
    @SerialName("android") Android,
    @SerialName("ios") Ios,
    @SerialName("web") Web,
}

@Serializable
data class ProtocolVersion(val major: Int, val minor: Int)

@Serializable
data class ProtocolRange(val min: ProtocolVersion, val max: ProtocolVersion) {
    companion object {
        val EXACT_V0 = ProtocolRange(ProtocolVersion(0, 0), ProtocolVersion(0, 0))
    }
}

@Serializable
data class ClientHello(
    val protocol: ProtocolRange,
    @SerialName("client_id") val clientId: String,
    @SerialName("client_name") val clientName: String,
    @SerialName("client_kind") val clientKind: ClientKind,
    @SerialName("requested_capabilities") val requestedCapabilities: List<String> = emptyList(),
)

@Serializable
data class ServerHello(
    val protocol: ProtocolVersion,
    @SerialName("session_id") val sessionId: String,
    @SerialName("granted_capabilities") val grantedCapabilities: List<String> = emptyList(),
)

@Serializable
data class PairRequest(
    val hello: ClientHello,
    val token: String,
)

@Serializable
data class PairedResponse(
    val hello: ServerHello,
    @SerialName("credential_id") val credentialId: String,
    val secret: String,
)

@Serializable
data class AuthenticateRequest(
    val hello: ClientHello,
    @SerialName("workspace_id") val workspaceId: String,
    @SerialName("credential_id") val credentialId: String,
    val secret: String,
)

@Serializable
data class ProtocolError(
    val code: String,
    val message: String? = null,
    val retryable: Boolean = false,
)

@Serializable
data class RequestEnvelope(
    @SerialName("request_id") val requestId: String,
    val body: JsonElement,
)

@Serializable
data class ResponseEnvelope(
    @SerialName("request_id") val requestId: String,
    val body: JsonElement,
)

@Serializable
data class EventEnvelope(
    @SerialName("event_sequence") val eventSequence: Long,
    val body: JsonElement,
)

@Serializable
sealed interface ClientMessage {
    @Serializable
    @SerialName("auth.pair")
    data class Pair(val payload: PairRequest) : ClientMessage

    @Serializable
    @SerialName("auth.authenticate")
    data class Authenticate(val payload: AuthenticateRequest) : ClientMessage

    @Serializable
    @SerialName("request")
    data class Request(val payload: RequestEnvelope) : ClientMessage
}

@Serializable
sealed interface ServerMessage {
    @Serializable
    @SerialName("auth.paired")
    data class Paired(val payload: PairedResponse) : ServerMessage

    @Serializable
    @SerialName("auth.authenticated")
    data class Authenticated(val payload: ServerHello) : ServerMessage

    @Serializable
    @SerialName("response")
    data class Response(val payload: ResponseEnvelope) : ServerMessage

    @Serializable
    @SerialName("event")
    data class Event(val payload: EventEnvelope) : ServerMessage

    @Serializable
    @SerialName("error")
    data class Error(val payload: ProtocolError) : ServerMessage
}
