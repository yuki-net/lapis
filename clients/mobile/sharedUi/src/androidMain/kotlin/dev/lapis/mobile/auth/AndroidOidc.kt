package dev.lapis.mobile.auth

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private var applicationContext: Context? = null

/** Android の Keystore / SharedPreferences を使う前に Application Context を渡す。 */
fun initializeAndroidOidc(context: Context) {
    applicationContext = context.applicationContext
}

private fun requireApplicationContext(): Context =
    applicationContext ?: error("Android OIDC storage is not initialized")

actual fun secureRandomBytes(size: Int): ByteArray =
    ByteArray(size).also { SecureRandom().nextBytes(it) }

actual fun openBrowser(url: String) {
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    requireApplicationContext().startActivity(intent)
}

actual fun createSecureTokenStore(): SecureTokenStore =
    AndroidSecureTokenStore(requireApplicationContext())

private class AndroidSecureTokenStore(context: Context) : SecureTokenStore {
    private val preferences =
        context.getSharedPreferences("lapis.oidc", Context.MODE_PRIVATE)

    override fun save(key: String, token: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        val encrypted = cipher.doFinal(token.encodeToByteArray())
        val iv = cipher.iv
        val packed = ByteArray(iv.size + encrypted.size)
        iv.copyInto(packed)
        encrypted.copyInto(packed, iv.size)
        preferences.edit()
            .putString(key, Base64.encodeToString(packed, Base64.NO_WRAP))
            .apply()
    }

    override fun load(key: String): String? {
        val value = preferences.getString(key, null) ?: return null
        return runCatching {
            val packed = Base64.decode(value, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(
                Cipher.DECRYPT_MODE,
                secretKey(),
                GCMParameterSpec(128, packed, 0, IV_SIZE),
            )
            cipher.doFinal(packed, IV_SIZE, packed.size - IV_SIZE).decodeToString()
        }.getOrNull()
    }

    override fun delete(key: String) {
        preferences.edit().remove(key).apply()
    }

    private fun secretKey(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator =
            KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build()
        )
        return generator.generateKey()
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "lapis.oidc.tokens"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_SIZE = 12
    }
}
