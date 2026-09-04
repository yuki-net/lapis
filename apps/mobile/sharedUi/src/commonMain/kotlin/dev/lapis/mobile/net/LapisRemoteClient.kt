package dev.lapis.mobile.net

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

    private val _events = MutableSharedFlow<EventEnvelope>(extraBufferCapacity = 64)
    val events: SharedFlow<EventEnvelope> = _events.asSharedFlow()

    private val pendingRequests = mutableMapOf<String, CompletableDeferred<ResponseEnvelope>>()
    private var nextRequestId = 1L

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
        try {
            client.webSocket(host = host, port = port, path = "/remote") {
                session = this
                val authMsg = ClientMessage.Authenticate(
                    AuthenticateRequest(
                        hello = ClientHello(
                            protocol = ProtocolRange.EXACT_V0,
                            clientId = clientId,
                            clientName = clientName,
                            clientKind = clientKind,
                        ),
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

    private suspend fun handleServerMessage(message: ServerMessage) {
        when (message) {
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
                _status.value = ConnectionStatus.Error(message.payload.message ?: message.payload.code)
            }
        }
    }

    suspend fun sendRequest(body: JsonElement): ResponseEnvelope {
        val currentSession = session ?: throw IllegalStateException("Not connected")
        val reqId = "req-${nextRequestId++}"
        val envelope = RequestEnvelope(requestId = reqId, body = body)
        val msg = ClientMessage.Request(envelope)
        val deferred = CompletableDeferred<ResponseEnvelope>()
        pendingRequests[reqId] = deferred

        currentSession.send(Frame.Text(lapisJson.encodeToString<ClientMessage>(msg)))
        return deferred.await()
    }
}
