package dev.tapas.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.Logout
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Sync
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.GrantStatus
import dev.tapas.api.models.Me
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private val shortTime = DateTimeFormatter.ofPattern("EEE d MMM HH:mm")

private fun OffsetDateTime.local(): String =
    atZoneSameInstant(ZoneId.systemDefault()).format(shortTime)

/** The signed-in account, each Google API's sync status, and sign out. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AccountScreen(me: Me) {
    val actions = LocalActions.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Account") },
                navigationIcon = {
                    IconButton(onClick = actions.back) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(me.email, style = MaterialTheme.typography.titleLarge)
            Text("Sync", style = MaterialTheme.typography.titleMedium)
            Column { me.grants.forEach { GrantRow(it) } }
            FilledTonalButton(onClick = actions.onContinueWithGoogle, modifier = Modifier.fillMaxWidth()) {
                Icon(Icons.Rounded.Sync, contentDescription = null)
                Text("Reconnect Google", Modifier.padding(start = 8.dp))
            }
            OutlinedButton(onClick = actions.viewModel::signOut, modifier = Modifier.fillMaxWidth()) {
                Icon(Icons.AutoMirrored.Rounded.Logout, contentDescription = null)
                Text("Sign out", Modifier.padding(start = 8.dp))
            }
        }
    }
}

/** One Google API: when it last synced, or why it failed. */
@Composable
private fun GrantRow(grant: GrantStatus) {
    val name = grant.api.value.replaceFirstChar(Char::uppercase)
    val failed = grant.syncError != null || grant.needsConsent
    ListItem(
        supportingContent = {
            Text(
                when {
                    grant.needsConsent -> "Google revoked access; reconnect to resume"
                    grant.syncError != null -> grant.syncError.orEmpty()
                    grant.syncedAt != null -> "Synced ${grant.syncedAt.local()}"
                    else -> "First sync running"
                },
                color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        },
        leadingContent = {
            Icon(
                when {
                    failed -> Icons.Rounded.Warning
                    grant.syncedAt != null -> Icons.Rounded.CheckCircle
                    else -> Icons.Rounded.Sync
                },
                contentDescription = null,
                tint = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
            )
        },
    ) { Text(name) }
}
