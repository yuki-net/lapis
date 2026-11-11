package dev.lapis.mobile.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement

val lapisJson = Json {
    ignoreUnknownKeys = true
    isLenient = true
    encodeDefaults = true
    explicitNulls = false
}

@Serializable
enum class ClientKind {
    @SerialName("desktop") Desktop,
    @SerialName("android") Android,
    @SerialName("ios") Ios,
}

@Serializable
enum class AuthMethod {
    @SerialName("token") Token,
    @SerialName("oidc") Oidc,
}

@Serializable
data class OidcClientConfig(
    val issuer: String,
    @SerialName("client_id") val clientId: String,
    val scopes: List<String> = emptyList(),
)

@Serializable
data class AuthRequired(
    val methods: List<AuthMethod>,
    val oidc: OidcClientConfig? = null,
)

@Serializable
data class ProtocolVersion(val major: Int, val minor: Int)

/// Rust `ProtocolRange` と同じ wire 形式。`min`/`max` ではなく範囲を直接持つ。
@Serializable
data class ProtocolRange(
    val major: Int,
    @SerialName("minimum_minor") val minimumMinor: Int,
    @SerialName("maximum_minor") val maximumMinor: Int,
) {
    companion object {
        val EXACT_V0 = ProtocolRange(0, 0, 0)

        fun exact(version: ProtocolVersion) =
            ProtocolRange(version.major, version.minor, version.minor)
    }
}

/** Rust `CURRENT_PROTOCOL` と揃える。 */
val CURRENT_PROTOCOL = ProtocolRange(0, 12, 12)

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
    val subject: String? = null,
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
data class OidcRequest(
    val hello: ClientHello,
    @SerialName("workspace_id") val workspaceId: String,
    @SerialName("id_token") val idToken: String,
)

@Serializable
data class ProtocolError(
    val code: String,
    val detail: String? = null,
    val retryable: Boolean = false,
    @SerialName("revision_conflict") val revisionConflict: JsonElement? = null,
)

/// Rust `RequestEnvelope` は `RequestBody` を flatten するため、`type`/`payload` を直接持つ。
@Serializable
data class RequestEnvelope(
    @SerialName("request_id") val requestId: String,
    val type: String,
    val payload: JsonElement,
)

@Serializable
data class ResponseEnvelope(
    @SerialName("request_id") val requestId: String,
    val type: String,
    val payload: JsonElement,
)

@Serializable
data class EventEnvelope(
    @SerialName("event_sequence") val eventSequence: Long,
    val type: String,
    val payload: JsonElement,
)

@Serializable
sealed interface ClientMessage {
    @Serializable
    @SerialName("hello")
    data class Hello(val payload: ClientHello) : ClientMessage

    @Serializable
    @SerialName("auth.pair")
    data class Pair(val payload: PairRequest) : ClientMessage

    @Serializable
    @SerialName("auth.authenticate")
    data class Authenticate(val payload: AuthenticateRequest) : ClientMessage

    @Serializable
    @SerialName("auth.oidc")
    data class Oidc(val payload: OidcRequest) : ClientMessage

    @Serializable
    @SerialName("request")
    data class Request(val payload: RequestEnvelope) : ClientMessage
}

@Serializable
sealed interface ServerMessage {
    @Serializable
    @SerialName("auth.required")
    data class AuthRequired(val payload: dev.lapis.mobile.protocol.AuthRequired) : ServerMessage

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
