package dev.lapis.mobile.auth

import dev.lapis.mobile.protocol.OidcClientConfig
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class OidcClientTest {
    @Test
    fun pkceChallengeMatchesRfc7636Vector() {
        assertEquals(
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            pkceChallenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        )
    }

    @Test
    fun callbackRequiresMatchingSchemeHostPathAndState() {
        assertEquals(
            CallbackResult.Code("abc"),
            parseCallback("lapis://auth/callback?code=abc&state=s1", "s1"),
        )
        assertEquals(
            CallbackResult.Ignore,
            parseCallback("lapis://auth/callback?code=abc&state=wrong", "s1"),
        )
        assertEquals(
            CallbackResult.Ignore,
            parseCallback("https://auth/callback?code=abc&state=s1", "s1"),
        )
        assertEquals(
            CallbackResult.Ignore,
            parseCallback("lapis://other/callback?code=abc&state=s1", "s1"),
        )
    }

    @Test
    fun callbackReportsIdpErrorAndMissingCode() {
        assertEquals(
            CallbackResult.Error("IdP がログインを拒否しました: access_denied"),
            parseCallback("lapis://auth/callback?error=access_denied&state=s1", "s1"),
        )
        assertEquals(
            CallbackResult.Error("OIDC callback に code がありません"),
            parseCallback("lapis://auth/callback?state=s1", "s1"),
        )
    }

    @Test
    fun authorizationUrlContainsPkceParameters() {
        val url = authorizationUrl(
            "https://idp.example/authorize",
            OidcClientConfig(
                issuer = "https://idp.example",
                clientId = "lapis-mobile",
                scopes = listOf("openid", "profile"),
            ),
            OIDC_REDIRECT_URI,
            "state-1",
            "challenge-1",
        )
        assertTrue(url.startsWith("https://idp.example/authorize?"))
        assertTrue(url.contains("response_type=code"))
        assertTrue(url.contains("client_id=lapis-mobile"))
        assertTrue(url.contains("redirect_uri=lapis%3A%2F%2Fauth%2Fcallback"))
        assertTrue(url.contains("scope=openid%20profile"))
        assertTrue(url.contains("state=state-1"))
        assertTrue(url.contains("code_challenge=challenge-1"))
        assertTrue(url.contains("code_challenge_method=S256"))
    }
}
