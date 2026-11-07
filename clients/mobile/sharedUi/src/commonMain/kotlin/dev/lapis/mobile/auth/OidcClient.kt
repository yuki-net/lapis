package dev.lapis.mobile.auth

import dev.lapis.mobile.protocol.OidcClientConfig
import dev.lapis.mobile.protocol.lapisJson
import io.ktor.client.HttpClient
import io.ktor.client.request.forms.FormDataContent
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsText
import io.ktor.http.HttpStatusCode
import io.ktor.http.Parameters
import io.ktor.http.Url
import io.ktor.http.encodeURLParameter
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import okio.ByteString.Companion.encodeUtf8

const val OIDC_REDIRECT_URI = "lapis://auth/callback"

private const val LOGIN_TIMEOUT_MS = 300_000L
private const val MAX_DISCOVERY_BYTES = 64 * 1024
private const val MAX_TOKEN_BYTES = 1024 * 1024

/** Authorization Code + PKCE のブラウザーログイン。 */
class OidcClient(private val http: HttpClient = HttpClient()) {
    private var pending: Pending? = null

    suspend fun login(
        config: OidcClientConfig,
        redirectUri: String = OIDC_REDIRECT_URI,
    ): String {
        val discovery = discover(config.issuer)
        val verifier = encodeBase64Url(secureRandomBytes(32))
        val state = encodeBase64Url(secureRandomBytes(16))
        val challenge = pkceChallenge(verifier)
        val deferred = CompletableDeferred<Result<String>>()
        pending = Pending(state, deferred)
        try {
            openBrowser(
                authorizationUrl(
                    discovery.authorizationEndpoint,
                    config,
                    redirectUri,
                    state,
                    challenge,
                )
            )
            val code = withTimeout(LOGIN_TIMEOUT_MS) { deferred.await().getOrThrow() }
            return exchange(
                discovery.tokenEndpoint,
                config.clientId,
                code,
                verifier,
                redirectUri,
            )
        } finally {
            pending = null
        }
    }

    /** Activity が受け取った callback URL を処理する。 */
    fun handleCallback(url: String) {
        val pending = pending ?: return
        when (val result = parseCallback(url, pending.state)) {
            is CallbackResult.Code -> pending.deferred.complete(Result.success(result.code))
            is CallbackResult.Error -> pending.deferred.complete(
                Result.failure(IllegalStateException(result.message))
            )
            CallbackResult.Ignore -> Unit
        }
    }

    /** 進行中のログインを完了できない状態にする。 */
    fun cancel() {
        pending?.deferred?.complete(
            Result.failure(IllegalStateException("OIDC ログインがキャンセルされました"))
        )
        pending = null
    }

    private suspend fun discover(issuer: String): Discovery {
        require(issuer.startsWith("https://")) { "OIDC issuer は HTTPS で指定してください" }
        val url = issuer.trimEnd('/') + "/.well-known/openid-configuration"
        val response = http.get(url)
        if (response.status != HttpStatusCode.OK) {
            error("OIDC discovery が ${response.status} を返しました")
        }
        val body = response.bodyAsText()
        if (body.length > MAX_DISCOVERY_BYTES) {
            error("OIDC discovery の応答が大きすぎます")
        }
        val discovery = lapisJson.decodeFromString<Discovery>(body)
        validateEndpoint("authorization_endpoint", discovery.authorizationEndpoint)
        validateEndpoint("token_endpoint", discovery.tokenEndpoint)
        return discovery
    }

    private suspend fun exchange(
        tokenEndpoint: String,
        clientId: String,
        code: String,
        verifier: String,
        redirectUri: String,
    ): String {
        val response = http.post(tokenEndpoint) {
            setBody(
                FormDataContent(
                    Parameters.build {
                        append("grant_type", "authorization_code")
                        append("code", code)
                        append("redirect_uri", redirectUri)
                        append("client_id", clientId)
                        append("code_verifier", verifier)
                    }
                )
            )
        }
        if (response.status != HttpStatusCode.OK) {
            error("OIDC token endpoint が ${response.status} を返しました")
        }
        val body = response.bodyAsText()
        if (body.length > MAX_TOKEN_BYTES) {
            error("OIDC token の応答が大きすぎます")
        }
        val token = lapisJson.decodeFromString<TokenResponse>(body)
        if (token.error != null) {
            error("OIDC token endpoint が code を拒否しました: ${token.error}")
        }
        return token.idToken?.takeIf { it.isNotEmpty() }
            ?: error("OIDC token の応答に ID Token がありません")
    }
}

private class Pending(
    val state: String,
    val deferred: CompletableDeferred<Result<String>>,
)

@Serializable
private data class Discovery(
    @SerialName("authorization_endpoint") val authorizationEndpoint: String,
    @SerialName("token_endpoint") val tokenEndpoint: String,
)

@Serializable
private data class TokenResponse(
    @SerialName("id_token") val idToken: String? = null,
    val error: String? = null,
)

internal sealed interface CallbackResult {
    data class Code(val code: String) : CallbackResult
    data class Error(val message: String) : CallbackResult
    data object Ignore : CallbackResult
}

internal fun parseCallback(url: String, expectedState: String): CallbackResult {
    val parsed = runCatching { Url(url) }.getOrNull() ?: return CallbackResult.Ignore
    if (parsed.protocol.name != "lapis" || parsed.host != "auth" || parsed.encodedPath != "/callback") {
        return CallbackResult.Ignore
    }
    if (parsed.parameters["state"] != expectedState) {
        return CallbackResult.Ignore
    }
    val error = parsed.parameters["error"]
    if (error != null) {
        return CallbackResult.Error("IdP がログインを拒否しました: $error")
    }
    val code = parsed.parameters["code"]
    if (code.isNullOrEmpty()) {
        return CallbackResult.Error("OIDC callback に code がありません")
    }
    return CallbackResult.Code(code)
}

internal fun authorizationUrl(
    endpoint: String,
    config: OidcClientConfig,
    redirectUri: String,
    state: String,
    challenge: String,
): String {
    val scopes = config.scopes.ifEmpty { listOf("openid", "profile") }
    val query = listOf(
        "response_type" to "code",
        "client_id" to config.clientId,
        "redirect_uri" to redirectUri,
        "scope" to scopes.joinToString(" "),
        "state" to state,
        "code_challenge" to challenge,
        "code_challenge_method" to "S256",
    ).joinToString("&") { "${it.first}=${it.second.encodeURLParameter()}" }
    val separator = if (endpoint.contains('?')) "&" else "?"
    return endpoint + separator + query
}

@OptIn(ExperimentalEncodingApi::class)
internal fun pkceChallenge(verifier: String): String =
    encodeBase64Url(verifier.encodeUtf8().sha256().toByteArray())

@OptIn(ExperimentalEncodingApi::class)
private fun encodeBase64Url(bytes: ByteArray): String =
    Base64.UrlSafe.encode(bytes).trimEnd('=')

private fun validateEndpoint(name: String, value: String) {
    val url = runCatching { Url(value) }.getOrNull() ?: error("OIDC $name が不正です")
    if (url.protocol.name != "https" || url.host.isEmpty() || url.fragment.isNotEmpty()) {
        error("OIDC $name は HTTPS URL で指定してください")
    }
}

expect fun secureRandomBytes(size: Int): ByteArray

expect fun openBrowser(url: String)
