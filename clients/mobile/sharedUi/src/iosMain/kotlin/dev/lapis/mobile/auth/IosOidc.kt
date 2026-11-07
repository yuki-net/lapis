package dev.lapis.mobile.auth

import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.addressOf
import kotlinx.cinterop.usePinned
import platform.Foundation.NSURL
import platform.Security.SecRandomCopyBytes
import platform.Security.kSecRandomDefault
import platform.UIKit.UIApplication

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
actual fun openBrowser(url: String) {
    val target = NSURL(string = url)
    UIApplication.sharedApplication.openURL(target)
}

actual fun createSecureTokenStore(): SecureTokenStore =
    error("iOS の secure storage は未実装です")
