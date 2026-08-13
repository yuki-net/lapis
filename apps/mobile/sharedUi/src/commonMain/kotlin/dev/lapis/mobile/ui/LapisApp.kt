package dev.lapis.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

private val LapisBackground = Color(0xFF111318)
private val LapisSurface = Color(0xFF191C22)
private val LapisAccent = Color(0xFF7AA2F7)
internal const val ApplicationName = "Lapis"

@Composable
fun LapisApp() {
    MaterialTheme {
        Surface(color = LapisBackground) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(LapisBackground)
                    .padding(24.dp),
                contentAlignment = Alignment.Center,
            ) {
                Column(
                    modifier = Modifier
                        .background(LapisSurface)
                        .padding(horizontal = 28.dp, vertical = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(ApplicationName, color = LapisAccent, style = MaterialTheme.typography.headlineMedium)
                    Text(
                        "Mobile client foundation is ready.",
                        color = Color(0xFFD9DEE8),
                        style = MaterialTheme.typography.bodyLarge,
                    )
                    Text(
                        "Protocol and backend connection will be added next.",
                        color = Color(0xFF8D96A8),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }
    }
}
