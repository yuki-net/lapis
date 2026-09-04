package dev.lapis.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.lapis.mobile.net.ConnectionStatus
import dev.lapis.mobile.net.LapisRemoteClient

private val LapisBackground = Color(0xFF111318)
private val LapisSurface = Color(0xFF191C22)
private val LapisAccent = Color(0xFF7AA2F7)
private val LapisTextMuted = Color(0xFF8D96A8)
internal const val ApplicationName = "Lapis"

enum class MobileScreen {
    Connection,
    Files,
    Editor,
    Terminal,
    Settings,
}

@Composable
fun LapisApp(remoteClient: LapisRemoteClient = remember { LapisRemoteClient() }) {
    var currentScreen by remember { mutableStateOf(MobileScreen.Connection) }
    val connectionStatus by remoteClient.status.collectAsState()

    MaterialTheme {
        Surface(color = LapisBackground) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .background(LapisBackground)
            ) {
                TabRow(
                    selectedTabIndex = currentScreen.ordinal,
                    containerColor = LapisSurface,
                    contentColor = LapisAccent,
                ) {
                    MobileScreen.entries.forEach { screen ->
                        Tab(
                            selected = currentScreen == screen,
                            onClick = { currentScreen = screen },
                            text = { Text(screen.name) },
                        )
                    }
                }

                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(16.dp),
                ) {
                    when (currentScreen) {
                        MobileScreen.Connection -> ConnectionScreen(connectionStatus)
                        MobileScreen.Files -> FilesScreen()
                        MobileScreen.Editor -> EditorScreen()
                        MobileScreen.Terminal -> TerminalScreen()
                        MobileScreen.Settings -> SettingsScreen()
                    }
                }
            }
        }
    }
}

@Composable
fun ConnectionScreen(status: ConnectionStatus) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(LapisSurface)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Remote Connection", color = LapisAccent, style = MaterialTheme.typography.titleLarge)
        Text("Status: $status", color = Color(0xFFD9DEE8))

        var host by remember { mutableStateOf("127.0.0.1") }
        var port by remember { mutableStateOf("8080") }
        var workspaceId by remember { mutableStateOf("workspace-default") }

        TextField(
            value = host,
            onValueChange = { host = it },
            label = { Text("Host") },
            modifier = Modifier.fillMaxWidth(),
        )
        TextField(
            value = port,
            onValueChange = { port = it },
            label = { Text("Port") },
            modifier = Modifier.fillMaxWidth(),
        )
        TextField(
            value = workspaceId,
            onValueChange = { workspaceId = it },
            label = { Text("Workspace ID") },
            modifier = Modifier.fillMaxWidth(),
        )

        Button(
            onClick = { /* Connect action triggered */ },
            modifier = Modifier.align(Alignment.End),
        ) {
            Text("Connect")
        }
    }
}

@Composable
fun FilesScreen() {
    Column(
        modifier = Modifier.fillMaxSize().background(LapisSurface).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Workspace Files", color = LapisAccent, style = MaterialTheme.typography.titleMedium)
        Text("No files loaded. Connect to a backend workspace to view files.", color = LapisTextMuted)
    }
}

@Composable
fun EditorScreen() {
    Column(
        modifier = Modifier.fillMaxSize().background(LapisSurface).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Document Editor", color = LapisAccent, style = MaterialTheme.typography.titleMedium)
        Text("No active document.", color = LapisTextMuted)
    }
}

@Composable
fun TerminalScreen() {
    Column(
        modifier = Modifier.fillMaxSize().background(LapisSurface).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Remote Terminal", color = LapisAccent, style = MaterialTheme.typography.titleMedium)
        Text("Terminal session is inactive.", color = LapisTextMuted)
    }
}

@Composable
fun SettingsScreen() {
    Column(
        modifier = Modifier.fillMaxSize().background(LapisSurface).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Settings", color = LapisAccent, style = MaterialTheme.typography.titleMedium)
        Text("Theme: Dark (Default)", color = Color(0xFFD9DEE8))
        Text("Language: Japanese / English", color = Color(0xFFD9DEE8))
    }
}
