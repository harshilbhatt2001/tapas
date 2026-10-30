package dev.tapas.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
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
    onContinueWithGoogle: () -> Unit,
    onSignOut: () -> Unit,
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
                is Screen.SignedOut -> {
                    if (screen.continueAutomatically) {
                        LaunchedEffect(screen) { onContinueWithGoogle() }
                    }
                    Box(Modifier.fillMaxSize(), Alignment.Center) {
                        Button(onClick = onContinueWithGoogle) { Text("Continue with Google") }
                    }
                }
                is Screen.Home -> Home(screen.me, screen.recent, onContinueWithGoogle, onSignOut)
            }
        }
    }
}

@Composable
private fun Home(me: Me, recent: Recent, onRefresh: () -> Unit, onSignOut: () -> Unit) {
    LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(me.email, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                TextButton(onClick = onRefresh) { Text("Refresh") }
                TextButton(onClick = onSignOut) { Text("Sign out") }
            }
        }
        item { SyncStatus(me.grants) }
        item { Text("Weights", style = MaterialTheme.typography.titleMedium) }
        items(recent.measurements) { MeasurementRow(it) }
        item { Text("Workouts", style = MaterialTheme.typography.titleMedium) }
        items(recent.workouts) { WorkoutRow(it) }
    }
}

/** One quiet line per Google API; errors only when a sync failed. */
@Composable
private fun SyncStatus(grants: List<GrantStatus>) {
    Column {
        for (grant in grants) {
            val name = grant.api.value.replaceFirstChar(Char::uppercase)
            when {
                grant.syncError != null ->
                    Text("$name: ${grant.syncError}", color = MaterialTheme.colorScheme.error)
                grant.syncedAt != null -> Text(
                    "$name synced ${grant.syncedAt.local()}",
                    style = MaterialTheme.typography.bodySmall,
                )
                else -> Text("$name: first sync running", style = MaterialTheme.typography.bodySmall)
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
