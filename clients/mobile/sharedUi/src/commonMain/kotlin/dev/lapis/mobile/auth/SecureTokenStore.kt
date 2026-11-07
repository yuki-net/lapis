package dev.lapis.mobile.auth

/** ID Token を OS の secure storage に保存する境界。 */
interface SecureTokenStore {
    fun save(key: String, token: String)
    fun load(key: String): String?
    fun delete(key: String)
}

expect fun createSecureTokenStore(): SecureTokenStore
