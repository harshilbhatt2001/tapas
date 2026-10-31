package dev.tapas.app

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeFlexibleTopAppBar
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MediumFlexibleTopAppBar
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.material3.toPath
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Matrix
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.Morph
import dev.tapas.api.models.Aggregation
import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalKind
import dev.tapas.api.models.MasterVerdict
import dev.tapas.api.models.Mode
import dev.tapas.api.models.NodeStatus
import dev.tapas.api.models.Observation
import kotlin.math.roundToInt

/** Home: the master's verdict, then the goal tree with each node's state. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CourseScreen(home: HomeData, refreshing: Boolean) {
    val actions = LocalActions.current
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    val statuses = home.course?.nodes.orEmpty().associateBy { it.goalId }
    var collapsed by rememberSaveable { mutableStateOf(setOf<Long>()) }
    val master = home.goals.master()
    val rows = home.goals.treeRows(collapsed).filter { it.goal.kind != GoalKind.master }
    val pull = rememberPullToRefreshState()

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            MediumFlexibleTopAppBar(
                title = { Text("Course") },
                subtitle = master?.let { { Text(it.title) } },
                actions = { AccountAction() },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = actions.viewModel::refresh,
            modifier = Modifier.padding(padding).fillMaxSize(),
            state = pull,
            indicator = {
                PullToRefreshDefaults.LoadingIndicator(
                    state = pull,
                    isRefreshing = refreshing,
                    modifier = Modifier.align(Alignment.TopCenter),
                )
            },
        ) {
            LazyColumn(
                contentPadding = PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                when (home.course) {
                    null -> item { Notice("The course is unavailable", home.courseError.orEmpty()) }
                    else -> item { Verdict(home.course.master, home.goals) }
                }
                if (rows.isEmpty()) item { NoGoals() }
                if (rows.isNotEmpty()) {
                    item {
                        Text(
                            "Goals",
                            style = MaterialTheme.typography.titleMedium,
                            modifier = Modifier.padding(top = 16.dp, bottom = 4.dp),
                        )
                    }
                }
                items(rows, key = { it.goal.id }) { row ->
                    NodeRow(
                        row = row.copy(depth = row.depth - if (master != null) 1 else 0),
                        status = statuses[row.goal.id],
                        expanded = row.goal.id !in collapsed,
                        onToggle = {
                            val id = row.goal.id
                            collapsed = if (id in collapsed) collapsed - id else collapsed + id
                        },
                        onOpen = { actions.navigate(Route.Node(row.goal.id)) },
                        modifier = Modifier.animateItem(),
                    )
                }
            }
        }
    }
}

@Composable
private fun NoGoals() {
    val actions = LocalActions.current
    Notice(
        "No goals yet",
        "Hang the goals that make up your target state under it, then split them into checkpoints.",
    ) {
        FilledTonalButton(onClick = { actions.navigate(Route.Goals) }) { Text("Build the tree") }
    }
}

@Composable
fun Notice(title: String, body: String, action: (@Composable () -> Unit)? = null) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            Text(body, style = MaterialTheme.typography.bodyMedium)
            action?.invoke()
        }
    }
}

/**
 * The master's verdict as the hero card: the tree's weighted days ahead or behind, the week's
 * mode as a shape that morphs between A and B, and needed against capacity hours.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Verdict(verdict: MasterVerdict, goals: List<Goal>) {
    val scheme = MaterialTheme.colorScheme
    ElevatedCard(
        Modifier.fillMaxWidth().animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec()),
        colors = CardDefaults.elevatedCardColors(containerColor = scheme.surfaceContainerLow),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                ModeBadge(verdict.mode)
                Column(Modifier.weight(1f)) {
                    Text(
                        daysPhrase(verdict.daysBehind).replaceFirstChar(Char::uppercase),
                        style = MaterialTheme.typography.headlineLargeEmphasized,
                    )
                    Text(
                        when (verdict.mode) {
                            Mode.a -> "Mode A · the full plan"
                            Mode.b -> "Mode B · the minimum viable fallback"
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = scheme.onSurfaceVariant,
                    )
                }
            }
            Capacity(verdict)
            if (verdict.overcommitted) {
                Surface(
                    color = scheme.errorContainer,
                    contentColor = scheme.onErrorContainer,
                    shape = MaterialTheme.shapes.large,
                ) {
                    Row(
                        Modifier.fillMaxWidth().padding(12.dp),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.Rounded.Warning, contentDescription = null)
                        Text(
                            "Overcommitted: the active goals need more hours than the week has. " +
                                "No single goal can fix this; defer one or re-weight.",
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
            }
            val drifting = verdict.drifting.mapNotNull { id -> goals.firstOrNull { it.id == id }?.title }
            if (drifting.isNotEmpty()) {
                Text(
                    "Drifting: ${drifting.joinToString()}",
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Capacity(verdict: MasterVerdict) {
    val scheme = MaterialTheme.colorScheme
    val capacity = verdict.capacityHours
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            if (capacity == null) {
                "${verdict.neededHours.short()} h a week needed · set a capacity on the target state"
            } else {
                "${verdict.neededHours.short()} h needed of ${capacity.short()} h a week"
            },
            style = MaterialTheme.typography.titleMedium,
        )
        if (capacity != null && capacity > 0) {
            val fraction = (verdict.neededHours / capacity).toFloat()
            val progress by animateFloatAsState(
                fraction.coerceIn(0f, 1f),
                MaterialTheme.motionScheme.slowSpatialSpec(),
            )
            LinearWavyProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth().semantics {
                    stateDescription = "${(fraction * 100).roundToInt()} percent of capacity"
                },
                color = if (verdict.overcommitted) scheme.error else scheme.primary,
            )
        }
    }
}

/** Mode A as a nine-sided cookie, Mode B as a four-leaf clover; a change morphs between them. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ModeBadge(mode: Mode) {
    val morph = remember { Morph(MaterialShapes.Cookie9Sided, MaterialShapes.Clover4Leaf) }
    val progress by animateFloatAsState(
        if (mode == Mode.a) 0f else 1f,
        MaterialTheme.motionScheme.slowSpatialSpec(),
    )
    val scheme = MaterialTheme.colorScheme
    Surface(
        Modifier.size(72.dp).rotate(progress * 90f),
        shape = MorphShape(morph, progress),
        color = if (mode == Mode.a) scheme.primary else scheme.tertiary,
        contentColor = if (mode == Mode.a) scheme.onPrimary else scheme.onTertiary,
    ) {
        Box(contentAlignment = Alignment.Center) {
            Text(
                mode.value.uppercase(),
                style = MaterialTheme.typography.headlineMediumEmphasized,
                modifier = Modifier.rotate(-progress * 90f),
            )
        }
    }
}

/** A [Morph] frozen at [progress], scaled to the component's size. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
private class MorphShape(private val morph: Morph, private val progress: Float) : Shape {
    override fun createOutline(size: Size, layoutDirection: LayoutDirection, density: Density): Outline {
        val path = morph.toPath(progress)
        path.transform(Matrix().apply { scale(size.width, size.height) })
        return Outline.Generic(path)
    }
}

/** One goal in the tree: indented by depth, collapsible when it has checkpoints. */
@Composable
private fun NodeRow(
    row: TreeRow,
    status: NodeStatus?,
    expanded: Boolean,
    onToggle: () -> Unit,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val turn by animateFloatAsState(if (expanded) 0f else -90f, MaterialTheme.motionScheme.fastSpatialSpec())
    Surface(
        modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.surfaceContainer,
    ) {
        Row(
            Modifier
                .clickable(onClick = onOpen, onClickLabel = "Open ${row.goal.title}", role = Role.Button)
                .heightIn(min = 64.dp)
                .padding(start = (row.depth * 20).dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (row.hasChildren) {
                IconButton(onClick = onToggle) {
                    Icon(
                        Icons.Rounded.ExpandMore,
                        contentDescription = if (expanded) "Collapse checkpoints" else "Expand checkpoints",
                        modifier = Modifier.rotate(turn),
                    )
                }
            } else {
                Spacer(Modifier.width(48.dp))
            }
            Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                Text(
                    row.goal.title,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                val detail = status?.daysBehind?.let(::daysPhrase) ?: row.goal.due?.let { "due ${it.medium()}" }
                detail?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            status?.let { StateChip(it.state, Modifier.padding(start = 8.dp)) }
        }
    }
}

/**
 * One goal in detail: its path against actuals, loop terms, arrival against due, what it needs
 * per week, its slack, any conflict, and its checkpoint stack.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun NodeScreen(home: HomeData, goalId: Long) {
    val actions = LocalActions.current
    val goal = home.goals.firstOrNull { it.id == goalId }
    val status = home.course?.nodes?.firstOrNull { it.goalId == goalId }
    val series = home.series.firstOrNull { it.id == goal?.seriesId }
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            LargeFlexibleTopAppBar(
                title = { Text(goal?.title.orEmpty(), maxLines = 2, overflow = TextOverflow.Ellipsis) },
                subtitle = home.goals.firstOrNull { it.id == goal?.parentId }?.let { { Text("in ${it.title}") } },
                navigationIcon = {
                    IconButton(onClick = actions.back) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    if (goal != null) {
                        IconButton(onClick = { actions.navigate(Route.GoalEditor(goal.id)) }) {
                            Icon(Icons.Rounded.Edit, contentDescription = "Edit goal")
                        }
                    }
                },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        if (goal == null) {
            Box(Modifier.padding(padding).fillMaxSize(), Alignment.Center) { Text("This goal is gone.") }
            return@Scaffold
        }
        Column(
            Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                status?.let { StateChip(it.state) }
                status?.daysBehind?.let {
                    Text(daysPhrase(it), style = MaterialTheme.typography.titleLarge)
                }
            }
            status?.conflict?.let { Notice("Conflict", it) }
            if (goal.kind == GoalKind.controlled && status != null && series != null) {
                ActualChart(goal, status, series.id, series.unit, series.aggregation)
            }
            Facts(goal, status, series?.unit)
            Checkpoints(home, goal)
        }
    }
}

@Composable
private fun ActualChart(goal: Goal, status: NodeStatus, seriesId: Long, unit: String, aggregation: Aggregation) {
    val actions = LocalActions.current
    var weeks by remember(seriesId) { mutableStateOf<List<Observation>?>(null) }
    LaunchedEffect(seriesId, status) {
        weeks = actions.viewModel.seriesWeeks(seriesId).getOrDefault(emptyList())
    }
    Card(Modifier.fillMaxWidth()) {
        Box(Modifier.padding(16.dp).heightIn(min = 120.dp), contentAlignment = Alignment.Center) {
            when (val loaded = weeks) {
                null -> Loading()
                else -> PathChart(
                    path = status.path,
                    actual = actualOnPath(loaded, aggregation, status.path),
                    unit = unit,
                    due = goal.due,
                )
            }
        }
    }
}

@Composable
private fun Facts(goal: Goal, status: NodeStatus?, unit: String?) {
    val terms = status?.terms
    FlowRow(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
        maxItemsInEachRow = 3,
    ) {
        if (terms != null) {
            Stat("P · gap now", "${terms.p.short()} d", Modifier.weight(1f))
            Stat("I · mean gap", "${terms.i.short()} d", Modifier.weight(1f))
            Stat(
                "D · trend",
                "${terms.d.short()} d/wk",
                Modifier.weight(1f),
                detail = if (terms.d > 0) "falling behind" else if (terms.d < 0) "catching up" else "steady",
            )
        }
        status?.projectedArrival?.let {
            val late = goal.due?.let { due -> java.time.temporal.ChronoUnit.DAYS.between(due, it) }
            Stat(
                "Projected arrival",
                it.medium(),
                Modifier.weight(1f),
                detail = late?.let { days ->
                    when {
                        days > 0 -> "$days d after due"
                        days < 0 -> "${-days} d before due"
                        else -> "on the due date"
                    }
                },
            )
        }
        goal.due?.let { Stat("Due", it.medium(), Modifier.weight(1f)) }
        goal.doneOn?.let { Stat("Done on", it.medium(), Modifier.weight(1f)) }
        status?.neededPerWeek?.let {
            Stat("Needed per week", "${it.short()} ${unit.orEmpty()}".trim(), Modifier.weight(1f))
        }
        status?.slackDays?.let {
            Stat("Slack", "$it d", Modifier.weight(1f), detail = "before the parent's due date")
        }
        goal.weeklyHours?.let { Stat("Weekly time", "${it.short()} h", Modifier.weight(1f)) }
    }
}

/** The goal's checkpoints as a stack, lowest due first, with the active one marked. */
@Composable
private fun Checkpoints(home: HomeData, goal: Goal) {
    val actions = LocalActions.current
    val stack = home.goals.checkpoints(goal.id)
        .map { it to home.course?.nodes?.firstOrNull { node -> node.goalId == it.id }?.state }
    if (stack.isEmpty()) return
    val active = stack.activeCheckpoint()
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Checkpoints", style = MaterialTheme.typography.titleMedium)
        stack.forEachIndexed { index, (checkpoint, state) ->
            ListItem(
                onClick = { actions.navigate(Route.Node(checkpoint.id)) },
                overlineContent = if (checkpoint == active) ({ Text("Active") }) else null,
                supportingContent = checkpoint.due?.let { { Text("due ${it.medium()}") } },
                leadingContent = { Text("#${index + 1}", style = MaterialTheme.typography.titleMedium) },
                trailingContent = state?.let { { StateChip(it) } },
            ) { Text(checkpoint.title) }
        }
    }
}
