package dev.tapas.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.GoogleApi
import dev.tapas.api.models.GrantStatus
import dev.tapas.api.models.Measurement
import dev.tapas.api.models.Me
import dev.tapas.api.models.Recent
import dev.tapas.api.models.Workout
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private val shortTime = DateTimeFormatter.ofPattern("EEE d MMM HH:mm")

private fun OffsetDateTime.local(): String =
    atZoneSameInstant(ZoneId.systemDefault()).format(shortTime)

@Composable
fun TapasApp(
    state: UiState,
    onSignIn: () -> Unit,
    onSignOut: () -> Unit,
    onRefresh: () -> Unit,
    onConnect: (GoogleApi) -> Unit,
) {
    Scaffold { padding ->
        Column(Modifier.padding(padding).padding(16.dp).fillMaxSize()) {
            state.error?.let {
                Text(it, color = MaterialTheme.colorScheme.error)
            }
            when (val screen = state.screen) {
                Screen.Loading -> Box(Modifier.fillMaxSize(), Alignment.Center) {
                    CircularProgressIndicator()
                }
                Screen.SignedOut -> Box(Modifier.fillMaxSize(), Alignment.Center) {
                    Button(onClick = onSignIn) { Text("Sign in with Google") }
                }
                is Screen.Home -> Home(screen.me, screen.recent, onSignOut, onRefresh, onConnect)
            }
        }
    }
}

@Composable
private fun Home(
    me: Me,
    recent: Recent,
    onSignOut: () -> Unit,
    onRefresh: () -> Unit,
    onConnect: (GoogleApi) -> Unit,
) {
    LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(me.email, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                TextButton(onClick = onRefresh) { Text("Refresh") }
                TextButton(onClick = onSignOut) { Text("Sign out") }
            }
        }
        items(GoogleApi.entries) { api ->
            GrantCard(api, me.grants.find { it.api == api }, onConnect)
        }
        item { Text("Weights", style = MaterialTheme.typography.titleMedium) }
        items(recent.measurements) { MeasurementRow(it) }
        item { Text("Workouts", style = MaterialTheme.typography.titleMedium) }
        items(recent.workouts) { WorkoutRow(it) }
    }
}

@Composable
private fun GrantCard(api: GoogleApi, grant: GrantStatus?, onConnect: (GoogleApi) -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Google ${api.value}", style = MaterialTheme.typography.titleSmall)
                when {
                    grant == null -> Text("Not connected")
                    grant.syncError != null ->
                        Text(grant.syncError, color = MaterialTheme.colorScheme.error)
                    grant.syncedAt != null -> Text("Synced ${grant.syncedAt.local()}")
                    else -> Text("Connected ${grant.grantedAt.local()}, not synced yet")
                }
            }
            OutlinedButton(onClick = { onConnect(api) }) {
                Text(if (grant == null) "Connect" else "Reconnect")
            }
        }
    }
}

@Composable
private fun MeasurementRow(measurement: Measurement) {
    Text("${measurement.at.local()}  ${"%.1f".format(measurement.value)} kg")
}

@Composable
private fun WorkoutRow(workout: Workout) {
    val minutes = (workout.activeSecs ?: 0) / 60
    val km = workout.distanceM?.let { " · ${"%.1f".format(it / 1000)} km" }.orEmpty()
    Text("${workout.start.local()}  ${workout.activity} · $minutes min$km")
}
