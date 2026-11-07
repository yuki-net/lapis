package dev.lapis.mobile

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import dev.lapis.mobile.auth.initializeAndroidOidc
import dev.lapis.mobile.net.LapisRemoteClient
import dev.lapis.mobile.ui.LapisApp

class MainActivity : ComponentActivity() {
    private val remoteClient = LapisRemoteClient()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initializeAndroidOidc(applicationContext)
        handleOidcIntent(intent)
        setContent {
            LapisApp(remoteClient)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleOidcIntent(intent)
    }

    private fun handleOidcIntent(intent: Intent?) {
        intent?.dataString?.let(remoteClient::handleOidcCallback)
    }
}
