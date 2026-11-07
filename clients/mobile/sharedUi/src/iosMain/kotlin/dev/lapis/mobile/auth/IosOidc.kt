package dev.lapis.mobile.auth

import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.addressOf
import kotlinx.cinterop.usePinned
import kotlinx.coroutines.suspendCancellableCoroutine
import platform.Foundation.NSURL
import platform.Security.SecRandomCopyBytes
import platform.Security.kSecRandomDefault
import platform.UIKit.UIApplication
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

@OptIn(ExperimentalForeignApi::class)
actual fun secureRandomBytes(size: Int): ByteArray {
    val bytes = ByteArray(size)
    bytes.usePinned { pinned ->
        check(SecRandomCopyBytes(kSecRandomDefault, size.toULong(), pinned.addressOf(0)) == 0) {
            "secure random generation failed"
        }
    }
    return bytes
}

@OptIn(ExperimentalForeignApi::class)
actual suspend fun openBrowser(url: String) {
    require(url.startsWith("https://")) { "OIDC URL は HTTPS で指定してください" }
    val target = NSURL.URLWithString(url) ?: error("OIDC URL が不正です")
    suspendCancellableCoroutine { continuation ->
        UIApplication.sharedApplication.openURL(
            target,
            options = emptyMap<Any?, Any?>(),
            completionHandler = { success ->
                if (success) {
                    continuation.resume(Unit)
                } else {
                    continuation.resumeWithException(
                        IllegalStateException("ブラウザーを開けませんでした")
                    )
                }
            },
        )
    }
}

actual fun createSecureTokenStore(): SecureTokenStore =
    error("iOS の secure storage は未実装です")
