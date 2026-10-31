package dev.tapas.app

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Flag
import androidx.compose.material.icons.rounded.Timeline
import androidx.compose.material.icons.rounded.Today
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MediumExtendedFloatingActionButton
import androidx.compose.material3.MediumFlexibleTopAppBar
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.Aggregation
import dev.tapas.api.models.Feed
import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalKind
import dev.tapas.api.models.Series
import dev.tapas.api.models.SeriesInput
import java.time.LocalDate
import kotlinx.coroutines.launch

/** The tree for building: tap a goal to edit it, add checkpoints under any goal. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun GoalsScreen(home: HomeData) {
    val actions = LocalActions.current
    val master = home.goals.master()
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            MediumFlexibleTopAppBar(
                title = { Text("Goals") },
                actions = { AccountAction() },
                scrollBehavior = scroll,
            )
        },
        floatingActionButton = {
            MediumExtendedFloatingActionButton(
                onClick = { actions.navigate(Route.GoalEditor(parentId = master?.id)) },
                icon = { Icon(Icons.Rounded.Add, contentDescription = null) },
                text = { Text("New goal") },
            )
        },
    ) { padding ->
        LazyColumn(
            Modifier.padding(padding).fillMaxSize(),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 120.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            items(home.goals.treeRows(), key = { it.goal.id }) { row ->
                GoalRow(row, home.series, Modifier.animateItem())
            }
        }
    }
}

@Composable
private fun GoalRow(row: TreeRow, series: List<Series>, modifier: Modifier = Modifier) {
    val actions = LocalActions.current
    val goal = row.goal
    val master = goal.kind == GoalKind.master
    Surface(
        modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        color = with(MaterialTheme.colorScheme) { if (master) secondaryContainer else surfaceContainer },
    ) {
        Row(
            Modifier
                .clickable(onClickLabel = "Edit ${goal.title}", role = Role.Button) {
                    actions.navigate(Route.GoalEditor(goal.id))
                }
                .heightIn(min = 64.dp)
                .padding(start = 16.dp + (row.depth * 20).dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Icon(goal.kind.icon(), contentDescription = goal.kind.label())
            Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                Text(
                    goal.title,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    goal.summary(series),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            IconButton(onClick = { actions.navigate(Route.GoalEditor(parentId = goal.id)) }) {
                Icon(
                    Icons.Rounded.Add,
                    contentDescription = if (master) "Add goal" else "Add checkpoint to ${goal.title}",
                )
            }
        }
    }
}

private fun GoalKind.icon() = when (this) {
    GoalKind.master -> Icons.Rounded.Flag
    GoalKind.controlled -> Icons.Rounded.Timeline
    GoalKind.milestone -> Icons.Rounded.Today
}

private fun GoalKind.label() = when (this) {
    GoalKind.master -> "Target state"
    GoalKind.controlled -> "Controlled"
    GoalKind.milestone -> "Milestone"
}

/** One line on what the goal is: capacity, series path or due date. */
private fun Goal.summary(series: List<Series>): String = when (kind) {
    GoalKind.master -> capacityHours?.let { "${it.short()} h a week capacity" } ?: "No capacity set"
    GoalKind.controlled -> {
        val unit = series.firstOrNull { it.id == seriesId }
        listOfNotNull(
            unit?.name,
            targetValue?.let { "to ${it.short()} ${unit?.unit.orEmpty()}".trim() },
            due?.let { "by ${it.medium()}" },
        ).joinToString(" · ")
    }
    GoalKind.milestone -> listOfNotNull(
        due?.let { "due ${it.medium()}" },
        doneOn?.let { "done ${it.medium()}" },
    ).joinToString(" · ").ifEmpty { "Milestone" }
}

/**
 * Creates or edits one goal. The master shows only its title and capacity and cannot be deleted;
 * the server's refusal of a save shows above the form.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GoalEditorScreen(home: HomeData, goalId: Long?, parentId: Long?) {
    val actions = LocalActions.current
    val existing = home.goals.firstOrNull { it.id == goalId }
    var draft by rememberSaveable(goalId, stateSaver = GoalDraft.saver) {
        mutableStateOf(
            existing?.let(GoalDraft::of)
                ?: GoalDraft(kind = GoalKind.controlled, parentId = parentId ?: home.goals.master()?.id),
        )
    }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    var saving by rememberSaveable { mutableStateOf(false) }
    var confirmDelete by rememberSaveable { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val isMaster = draft.kind == GoalKind.master

    fun save() {
        val input = try {
            draft.toInput()
        } catch (invalid: IllegalArgumentException) {
            error = invalid.message
            return
        }
        saving = true
        scope.launch {
            actions.viewModel.saveGoal(goalId, input)
                .onSuccess { actions.back() }
                .onFailure { error = it.message }
            saving = false
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        when {
                            isMaster -> "Target state"
                            existing == null -> "New goal"
                            else -> "Edit goal"
                        },
                    )
                },
                navigationIcon = {
                    IconButton(onClick = actions.back) { Icon(Icons.Rounded.Close, contentDescription = "Discard") }
                },
                actions = {
                    TextButton(onClick = ::save, enabled = !saving) { Text("Save") }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            AnimatedVisibility(error != null) {
                Notice("Not saved", error.orEmpty())
            }
            OutlinedTextField(
                value = draft.title,
                onValueChange = { draft = draft.copy(title = it) },
                label = { Text("Title") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            if (isMaster) {
                NumberField(
                    "Weekly capacity",
                    draft.capacityHours,
                    { draft = draft.copy(capacityHours = it) },
                    suffix = "h",
                )
            } else {
                GoalFields(home, goalId, draft) { draft = it }
            }
            if (existing != null && !isMaster) {
                OutlinedButton(
                    onClick = { confirmDelete = true },
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(Icons.Rounded.Delete, contentDescription = null)
                    Text("Delete goal", Modifier.padding(start = 8.dp))
                }
            }
        }
    }

    if (confirmDelete && existing != null) {
        val below = home.goals.subtree(existing.id).size - 1
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            icon = { Icon(Icons.Rounded.Delete, contentDescription = null) },
            title = { Text("Delete ${existing.title}?") },
            text = {
                Text(
                    when (below) {
                        0 -> "This cannot be undone."
                        1 -> "Its checkpoint goes with it."
                        else -> "Its $below checkpoints go with it."
                    },
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmDelete = false
                        scope.launch {
                            actions.viewModel.deleteGoal(existing.id)
                                .onSuccess {
                                    val deleted = home.goals.subtree(existing.id)
                                    actions.popWhile {
                                        it is Route.GoalEditor || it is Route.Node && it.goalId in deleted
                                    }
                                }
                                .onFailure { error = it.message }
                        }
                    },
                ) { Text("Delete") }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }
}

/** Everything but the title for a controlled goal or milestone. */
@Composable
private fun GoalFields(home: HomeData, goalId: Long?, draft: GoalDraft, onDraft: (GoalDraft) -> Unit) {
    val excluded = goalId?.let(home.goals::subtree).orEmpty()
    val parents = home.goals.treeRows().map { it.goal }.filter { it.id !in excluded }
    Picker(
        label = "Parent",
        options = parents,
        selected = parents.firstOrNull { it.id == draft.parentId },
        onSelect = { onDraft(draft.copy(parentId = it.id)) },
        text = { it.title },
        supportingText = "Under a goal other than the target state, it is a checkpoint stacked by due date",
    )
    ChoiceGroup(
        options = listOf(GoalKind.controlled, GoalKind.milestone),
        selected = draft.kind,
        onSelect = { onDraft(draft.copy(kind = it)) },
        label = { it.label() },
        icon = { it.icon() },
    )
    DateField(
        "Start",
        draft.start,
        { onDraft(draft.copy(start = it)) },
        onClear = { onDraft(draft.copy(start = null)) },
        supportingText = "Leave empty to start where the previous checkpoint ends",
    )
    DateField("Due", draft.due, { onDraft(draft.copy(due = it)) }, onClear = { onDraft(draft.copy(due = null)) })
    when (draft.kind) {
        GoalKind.controlled -> ControlledFields(home.series, draft, onDraft)
        GoalKind.milestone -> MilestoneFields(draft, onDraft)
        GoalKind.master -> Unit
    }
    NumberField(
        "Weight",
        draft.weight,
        { onDraft(draft.copy(weight = it)) },
        supporting = "Share of the parent's error",
    )
    NumberField("Weekly time", draft.weeklyHours, { onDraft(draft.copy(weeklyHours = it)) }, suffix = "h")
}

@Composable
private fun ControlledFields(series: List<Series>, draft: GoalDraft, onDraft: (GoalDraft) -> Unit) {
    var creatingSeries by rememberSaveable { mutableStateOf(false) }
    val selected = series.firstOrNull { it.id == draft.seriesId }
    Picker(
        label = "Series",
        options = series,
        selected = selected,
        onSelect = { onDraft(draft.copy(seriesId = it.id)) },
        text = { "${it.name} (${it.unit})" },
        supportingText = when (selected?.aggregation) {
            Aggregation.sum -> "Weekly totals; values below are running totals since start"
            Aggregation.level -> "The week's last reading"
            null -> null
        },
        extra = listOf("New series…" to { creatingSeries = true }),
    )
    val unit = selected?.unit
    NumberField("Start value", draft.startValue, { onDraft(draft.copy(startValue = it)) }, suffix = unit)
    if (selected?.aggregation == Aggregation.sum) WeeklyRateHelper(draft, onDraft)
    NumberField("Target value", draft.targetValue, { onDraft(draft.copy(targetValue = it)) }, suffix = unit)
    NumberField(
        "Max ramp",
        draft.maxRampPercent,
        { onDraft(draft.copy(maxRampPercent = it)) },
        suffix = "%",
        supporting = "Largest week-on-week rise of the weekly amount",
    )
    if (creatingSeries) {
        NewSeriesDialog(
            onDismiss = { creatingSeries = false },
            onCreated = {
                creatingSeries = false
                onDraft(draft.copy(seriesId = it.id))
            },
        )
    }
}

/**
 * For a `sum` series, fills the target running total from a weekly amount: 5 h a week for 12
 * weeks targets 60 h. Needs both dates, since the total depends on the span.
 */
@Composable
private fun WeeklyRateHelper(draft: GoalDraft, onDraft: (GoalDraft) -> Unit) {
    val start = draft.start
    val due = draft.due
    val startValue = runCatching { number(draft.startValue, "") }.getOrNull() ?: 0.0
    var perWeek by rememberSaveable {
        val target = runCatching { number(draft.targetValue, "") }.getOrNull()
        val rate = if (target != null && start != null && due != null) {
            weeklyRateForTarget(target, startValue, start, due)
        } else {
            null
        }
        mutableStateOf(rate?.plain().orEmpty())
    }
    val ready = start != null && due != null && due.isAfter(start)
    NumberField(
        "Per week",
        perWeek,
        { text ->
            perWeek = text
            val rate = runCatching { number(text, "") }.getOrNull()
            if (rate != null && ready) {
                onDraft(draft.copy(targetValue = targetForWeeklyRate(rate, startValue, start, due).plain()))
            }
        },
        enabled = ready,
        supporting = if (ready) {
            "Fills the target for %s weeks".format(weeksBetween(start, due).short())
        } else {
            "Set start and due to fill the target from a weekly amount"
        },
    )
}

@Composable
private fun MilestoneFields(draft: GoalDraft, onDraft: (GoalDraft) -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Done", style = MaterialTheme.typography.bodyLarge)
            draft.doneOn?.let {
                Text(
                    "Ticked on ${it.medium()}",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Switch(
            checked = draft.doneOn != null,
            onCheckedChange = { done -> onDraft(draft.copy(doneOn = if (done) LocalDate.now() else null)) },
        )
    }
    draft.doneOn?.let { DateField("Done on", it, { day -> onDraft(draft.copy(doneOn = day)) }) }
}

@Composable
fun NumberField(
    label: String,
    value: String,
    onValue: (String) -> Unit,
    modifier: Modifier = Modifier,
    suffix: String? = null,
    supporting: String? = null,
    enabled: Boolean = true,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValue,
        label = { Text(label) },
        suffix = suffix?.let { { Text(it) } },
        supportingText = supporting?.let { { Text(it) } },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
        singleLine = true,
        enabled = enabled,
        modifier = modifier.fillMaxWidth(),
    )
}

/** Creates a manual series: a name, a unit, and whether a week is its last reading or its total. */
@Composable
fun NewSeriesDialog(onDismiss: () -> Unit, onCreated: (Series) -> Unit) {
    val actions = LocalActions.current
    val scope = rememberCoroutineScope()
    var name by rememberSaveable { mutableStateOf("") }
    var unit by rememberSaveable { mutableStateOf("") }
    var aggregation by rememberSaveable { mutableStateOf(Aggregation.level) }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    var saving by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New series") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    name,
                    { name = it },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    unit,
                    { unit = it },
                    label = { Text("Unit") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                ChoiceGroup(
                    options = Aggregation.entries,
                    selected = aggregation,
                    onSelect = { aggregation = it },
                    label = { if (it == Aggregation.level) "Level" else "Sum" },
                )
                Text(
                    if (aggregation == Aggregation.level) {
                        "A week is its last reading, like a salary or savings balance."
                    } else {
                        "A week is the total of its readings, like deep-work hours."
                    },
                    style = MaterialTheme.typography.bodySmall,
                )
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            Button(
                enabled = !saving && name.isNotBlank() && unit.isNotBlank(),
                onClick = {
                    saving = true
                    scope.launch {
                        val input = SeriesInput(name = name.trim(), unit = unit.trim(), aggregation = aggregation)
                        actions.viewModel.createSeries(input)
                            .onSuccess(onCreated)
                            .onFailure { error = it.message }
                        saving = false
                    }
                },
            ) { Text("Create") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Series the user logs by hand. */
val List<Series>.manual: List<Series> get() = filter { it.feed == Feed.manual }
