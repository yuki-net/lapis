package dev.lapis.mobile.net

import dev.lapis.mobile.auth.OidcClient
import dev.lapis.mobile.auth.SecureTokenStore
import dev.lapis.mobile.auth.createSecureTokenStore
import dev.lapis.mobile.protocol.*
import io.ktor.client.*
import io.ktor.client.plugins.websocket.*
import io.ktor.websocket.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement

sealed interface ConnectionStatus {
    data object Disconnected : ConnectionStatus
    data object Connecting : ConnectionStatus
    data class Authenticated(val serverHello: ServerHello) : ConnectionStatus
    data class Error(val message: String) : ConnectionStatus
}

class LapisRemoteClient(
    private val client: HttpClient = HttpClient {
        install(WebSockets)
    },
) {
    private var session: DefaultClientWebSocketSession? = null
    private val _status = MutableStateFlow<ConnectionStatus>(ConnectionStatus.Disconnected)
    val status: StateFlow<ConnectionStatus> = _status.asStateFlow()

    private val _authRequired = MutableStateFlow<AuthRequired?>(null)
    val authRequired: StateFlow<AuthRequired?> = _authRequired.asStateFlow()

    private val _events = MutableSharedFlow<EventEnvelope>(extraBufferCapacity = 64)
    val events: SharedFlow<EventEnvelope> = _events.asSharedFlow()

    private val pendingRequests = mutableMapOf<String, CompletableDeferred<ResponseEnvelope>>()
    private var nextRequestId = 1L
    private val oidcClient = OidcClient()
    private val tokenStore: SecureTokenStore by lazy { createSecureTokenStore() }

    /** OIDC callback URL を進行中のログインへ渡す。 */
    fun handleOidcCallback(url: String) {
        oidcClient.handleCallback(url)
    }

    suspend fun connectAndAuthenticate(
        host: String,
        port: Int,
        workspaceId: String,
        credentialId: String,
        secret: String,
        clientId: String = "mobile-kmp",
        clientName: String = "Lapis Mobile",
        clientKind: ClientKind = ClientKind.Android,
    ) {
        _status.value = ConnectionStatus.Connecting
        _authRequired.value = null
        try {
            client.webSocket(host = host, port = port, path = "/remote") {
                session = this
                val authMsg = ClientMessage.Authenticate(
                    AuthenticateRequest(
                        hello = hello(clientId, clientName, clientKind),
                        workspaceId = workspaceId,
                        credentialId = credentialId,
                        secret = secret,
                    )
                )
                send(Frame.Text(lapisJson.encodeToString<ClientMessage>(authMsg)))

                for (frame in incoming) {
                    if (frame is Frame.Text) {
                        val text = frame.readText()
                        val serverMsg = lapisJson.decodeFromString<ServerMessage>(text)
                        handleServerMessage(serverMsg)
                    }
                }
            }
        } catch (e: Exception) {
            _status.value = ConnectionStatus.Error(e.message ?: "Connection error")
        } finally {
            _status.value = ConnectionStatus.Disconnected
            session = null
        }
    }

    /**
     * OIDC でログインし、ID Token を secure storage に保存する。
     * Host の `AuthRequired.oidc` が必須。
     */
    suspend fun connectWithOidc(
        host: String,
        port: Int,
        workspaceId: String,
        clientId: String = "mobile-kmp",
        clientName: String = "Lapis Mobile",
        clientKind: ClientKind = ClientKind.Android,
    ) {
        _status.value = ConnectionStatus.Connecting
        _authRequired.value = null
        try {
            client.webSocket(host = host, port = port, path = "/remote") {
                session = this
                val helloMessage = hello(clientId, clientName, clientKind)
                send(
                    Frame.Text(
                        lapisJson.encodeToString<ClientMessage>(ClientMessage.Hello(helloMessage))
                    )
                )
                val first = (incoming.receive() as? Frame.Text)
                    ?.readText()
                    ?: error("Host が認証方式を返しません")
                val required = lapisJson.decodeFromString<ServerMessage>(first) as? ServerMessage.AuthRequired
                    ?: error("Host が認証方式を返しません")
                _authRequired.value = required.payload
                val oidc = required.payload.oidc
                    ?: error("Host が OIDC ログインを提示していません")
                val idToken = oidcClient.login(oidc)
                send(
                    Frame.Text(
                        lapisJson.encodeToString<ClientMessage>(
                            ClientMessage.Oidc(
                                OidcRequest(
                                    hello = helloMessage,
                                    workspaceId = workspaceId,
                                    idToken = idToken,
                                )
                            )
                        )
                    )
                )
                for (frame in incoming) {
                    if (frame is Frame.Text) {
                        val serverMsg = lapisJson.decodeFromString<ServerMessage>(frame.readText())
                        if (serverMsg is ServerMessage.Authenticated) {
                            serverMsg.payload.subject?.let { subject ->
                                runCatching {
                                    tokenStore.save(oidcTokenKey(host, port, subject), idToken)
                                    tokenStore.save(oidcCurrentKey(host, port), idToken)
                                }
                            }
                        }
                        handleServerMessage(serverMsg)
                    }
                }
            }
        } catch (e: Exception) {
            _status.value = ConnectionStatus.Error(e.message ?: "OIDC connection error")
        } finally {
            _status.value = ConnectionStatus.Disconnected
            session = null
        }
    }

    private fun hello(
        clientId: String,
        clientName: String,
        clientKind: ClientKind,
    ) = ClientHello(
        protocol = CURRENT_PROTOCOL,
        clientId = clientId,
        clientName = clientName,
        clientKind = clientKind,
    )

    private fun oidcTokenKey(host: String, port: Int, subject: String): String =
        "oidc|$host|$port|$subject"

    private fun oidcCurrentKey(host: String, port: Int): String =
        "oidc-current|$host|$port"

    private suspend fun handleServerMessage(message: ServerMessage) {
        when (message) {
            is ServerMessage.AuthRequired -> {
                _authRequired.value = message.payload
            }
            is ServerMessage.Authenticated -> {
                _status.value = ConnectionStatus.Authenticated(message.payload)
            }
            is ServerMessage.Paired -> {
                _status.value = ConnectionStatus.Authenticated(message.payload.hello)
            }
            is ServerMessage.Response -> {
                val deferred = pendingRequests.remove(message.payload.requestId)
                deferred?.complete(message.payload)
            }
            is ServerMessage.Event -> {
                _events.emit(message.payload)
            }
            is ServerMessage.Error -> {
                _status.value = ConnectionStatus.Error(message.payload.detail ?: message.payload.code)
            }
        }
    }

    suspend fun sendRequest(type: String, payload: JsonElement): ResponseEnvelope {
        val currentSession = session ?: throw IllegalStateException("Not connected")
        val reqId = "req-${nextRequestId++}"
        val envelope = RequestEnvelope(requestId = reqId, type = type, payload = payload)
        val msg = ClientMessage.Request(envelope)
        val deferred = CompletableDeferred<ResponseEnvelope>()
        pendingRequests[reqId] = deferred

        currentSession.send(Frame.Text(lapisJson.encodeToString<ClientMessage>(msg)))
        return deferred.await()
    }
}
