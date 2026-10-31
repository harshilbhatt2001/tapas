package dev.tapas.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MediumFlexibleTopAppBar
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.Aggregation
import dev.tapas.api.models.Observation
import java.time.LocalDate
import kotlinx.coroutines.launch

private const val RECENT_WEEKS = 12

/** Logs one day's value for a manual series and shows its recent weeks. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun LogScreen(home: HomeData) {
    val actions = LocalActions.current
    val manual = home.series.manual
    var seriesId by rememberSaveable { mutableStateOf(manual.firstOrNull()?.id) }
    val series = manual.firstOrNull { it.id == seriesId } ?: manual.firstOrNull()
    var day by rememberSaveable { mutableStateOf(LocalDate.now()) }
    var value by rememberSaveable { mutableStateOf("") }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    var saving by rememberSaveable { mutableStateOf(false) }
    var creatingSeries by rememberSaveable { mutableStateOf(false) }
    var weeks by remember(series?.id) { mutableStateOf<List<Observation>?>(null) }
    var logged by remember { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()

    LaunchedEffect(series?.id, logged) {
        weeks = series?.let { actions.viewModel.seriesWeeks(it.id).getOrDefault(emptyList()) }
    }

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            MediumFlexibleTopAppBar(title = { Text("Log") }, actions = { AccountAction() }, scrollBehavior = scroll)
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        LazyColumn(
            Modifier.padding(padding).fillMaxSize(),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            if (series == null) {
                item {
                    Notice(
                        "Nothing to log yet",
                        "Manual series hold what no sensor reads: salary, savings, deep-work hours.",
                    ) {
                        FilledTonalButton(onClick = { creatingSeries = true }) { Text("New series") }
                    }
                }
                return@LazyColumn
            }
            item {
                Picker(
                    label = "Series",
                    options = manual,
                    selected = series,
                    onSelect = { seriesId = it.id },
                    text = { "${it.name} (${it.unit})" },
                    extra = listOf("New series…" to { creatingSeries = true }),
                )
            }
            item { DateField("Day", day, { day = it }) }
            item {
                NumberField(
                    "Value",
                    value,
                    { value = it },
                    suffix = series.unit,
                    supporting = error ?: if (series.aggregation == Aggregation.sum) {
                        "Adds to the week's total; logging the same day again replaces it"
                    } else {
                        "The week counts its last reading; logging the same day again replaces it"
                    },
                )
            }
            item {
                Button(
                    enabled = !saving && value.isNotBlank(),
                    modifier = Modifier.fillMaxWidth(),
                    onClick = {
                        val number = try {
                            requireNotNull(number(value, "Value"))
                        } catch (invalid: IllegalArgumentException) {
                            error = invalid.message
                            return@Button
                        }
                        saving = true
                        scope.launch {
                            actions.viewModel.logEntry(series.id, day, number)
                                .onSuccess {
                                    error = null
                                    value = ""
                                    logged++
                                    snackbar.showSnackbar("Logged ${number.short()} ${series.unit} for ${day.medium()}")
                                }
                                .onFailure { error = it.message }
                            saving = false
                        }
                    },
                ) {
                    Icon(Icons.Rounded.Check, contentDescription = null)
                    Text("Log", Modifier.padding(start = 8.dp))
                }
            }
            item { Text("Recent weeks", style = MaterialTheme.typography.titleMedium) }
            when (val loaded = weeks) {
                null -> item { Box(Modifier.fillMaxWidth(), Alignment.Center) { Loading() } }
                else -> {
                    if (loaded.isEmpty()) item { Text("No entries yet.", style = MaterialTheme.typography.bodyMedium) }
                    items(loaded.takeLast(RECENT_WEEKS).reversed(), key = { it.week.toEpochDay() }) { week ->
                        ListItem(supportingContent = { Text("Week of ${week.week.medium()}") }) {
                            Text("${week.value.short()} ${series.unit}")
                        }
                    }
                }
            }
        }
    }
    if (creatingSeries) {
        NewSeriesDialog(
            onDismiss = { creatingSeries = false },
            onCreated = {
                creatingSeries = false
                seriesId = it.id
            },
        )
    }
}
