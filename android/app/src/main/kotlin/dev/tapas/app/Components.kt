package dev.tapas.app

import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.CallMerge
import androidx.compose.material.icons.automirrored.rounded.TrendingUp
import androidx.compose.material.icons.rounded.AccountCircle
import androidx.compose.material.icons.rounded.Alarm
import androidx.compose.material.icons.rounded.CalendarMonth
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Clear
import androidx.compose.material.icons.rounded.Event
import androidx.compose.material.icons.rounded.EventBusy
import androidx.compose.material.icons.rounded.HourglassEmpty
import androidx.compose.material.icons.rounded.SensorsOff
import androidx.compose.material.icons.rounded.TaskAlt
import androidx.compose.material.icons.automirrored.rounded.TrendingDown
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ExposedDropdownMenu
import androidx.compose.material3.ExposedDropdownMenuAnchorType
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.NodeState
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

/** How a node state looks: its label, icon and container colours. */
data class StateStyle(val label: String, val icon: ImageVector, val colors: ContainerColors)

@Composable
fun NodeState.style(): StateStyle {
    val scheme = MaterialTheme.colorScheme
    val quiet = ContainerColors(scheme.surfaceContainerHighest, scheme.onSurfaceVariant)
    val good = ContainerColors(scheme.primaryContainer, scheme.onPrimaryContainer)
    val finished = ContainerColors(scheme.primary, scheme.onPrimary)
    val broken = ContainerColors(scheme.errorContainer, scheme.onErrorContainer)
    return when (this) {
        NodeState.on_course -> StateStyle("On course", Icons.Rounded.CheckCircle, good)
        NodeState.ahead -> StateStyle(
            "Ahead",
            Icons.AutoMirrored.Rounded.TrendingUp,
            ContainerColors(scheme.tertiaryContainer, scheme.onTertiaryContainer),
        )
        NodeState.behind -> StateStyle("Behind", Icons.AutoMirrored.Rounded.TrendingDown, scheme.caution)
        NodeState.conflict -> StateStyle("Conflict", Icons.Rounded.Warning, broken)
        NodeState.sensor_stale -> StateStyle("No readings", Icons.Rounded.SensorsOff, quiet)
        NodeState.pending -> StateStyle("Pending", Icons.Rounded.HourglassEmpty, quiet)
        NodeState.merged -> StateStyle("Merged", Icons.AutoMirrored.Rounded.CallMerge, finished)
        NodeState.upcoming -> StateStyle("Upcoming", Icons.Rounded.Event, quiet)
        NodeState.due_soon -> StateStyle("Due soon", Icons.Rounded.Alarm, scheme.caution)
        NodeState.done -> StateStyle("Done", Icons.Rounded.TaskAlt, finished)
        NodeState.overdue -> StateStyle("Overdue", Icons.Rounded.EventBusy, broken)
    }
}

/** A state as a pill: icon and label, so it reads without relying on colour. */
@Composable
fun StateChip(state: NodeState, modifier: Modifier = Modifier) {
    val style = state.style()
    Surface(modifier, shape = CircleShape, color = style.colors.container, contentColor = style.colors.content) {
        Row(
            Modifier.padding(start = 8.dp, end = 12.dp, top = 4.dp, bottom = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(style.icon, contentDescription = null, Modifier.size(18.dp))
            Text(style.label, style = MaterialTheme.typography.labelLarge)
        }
    }
}

/** Opens the account and sync screen; sits in each destination's top bar. */
@Composable
fun AccountAction() {
    val actions = LocalActions.current
    IconButton(onClick = { actions.navigate(Route.Account) }) {
        Icon(Icons.Rounded.AccountCircle, contentDescription = "Account and sync")
    }
}

private val mediumDate = DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM)

fun LocalDate.medium(): String = format(mediumDate)

/**
 * A date as a read-only text field that opens the date picker when tapped. With [onClear] the
 * date is optional and a clear button empties it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DateField(
    label: String,
    date: LocalDate?,
    onDate: (LocalDate) -> Unit,
    modifier: Modifier = Modifier,
    onClear: (() -> Unit)? = null,
    supportingText: String? = null,
) {
    var picking by rememberSaveable { mutableStateOf(false) }
    val presses = remember { MutableInteractionSource() }
    LaunchedEffect(presses) {
        presses.interactions.collect { if (it is PressInteraction.Release) picking = true }
    }
    OutlinedTextField(
        value = date?.medium().orEmpty(),
        onValueChange = {},
        readOnly = true,
        label = { Text(label) },
        modifier = modifier.fillMaxWidth(),
        interactionSource = presses,
        supportingText = supportingText?.let { { Text(it) } },
        trailingIcon = {
            if (date != null && onClear != null) {
                IconButton(onClick = onClear) { Icon(Icons.Rounded.Clear, contentDescription = "Clear $label") }
            } else {
                IconButton(onClick = { picking = true }) {
                    Icon(Icons.Rounded.CalendarMonth, contentDescription = "Pick $label")
                }
            }
        },
    )
    if (picking) {
        val picker = rememberDatePickerState(
            initialSelectedDateMillis = (date ?: LocalDate.now()).atStartOfDay(ZoneOffset.UTC)
                .toInstant().toEpochMilli(),
        )
        DatePickerDialog(
            onDismissRequest = { picking = false },
            confirmButton = {
                TextButton(
                    onClick = {
                        picker.selectedDateMillis?.let {
                            onDate(Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate())
                        }
                        picking = false
                    },
                ) { Text("OK") }
            },
            dismissButton = { TextButton(onClick = { picking = false }) { Text("Cancel") } },
        ) { DatePicker(picker) }
    }
}

/**
 * A single choice as a connected button group: the expressive replacement for segmented buttons,
 * with the selected button morphing to a full pill.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun <T> ChoiceGroup(
    options: List<T>,
    selected: T,
    onSelect: (T) -> Unit,
    label: (T) -> String,
    modifier: Modifier = Modifier,
    icon: ((T) -> ImageVector)? = null,
) {
    Row(
        modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween),
    ) {
        options.forEachIndexed { index, option ->
            ToggleButton(
                checked = option == selected,
                onCheckedChange = { onSelect(option) },
                modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                icon = icon?.let { { Icon(it(option), contentDescription = null) } },
                shapes = when (index) {
                    0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                    options.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                    else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                },
            ) { Text(label(option), maxLines = 1) }
        }
    }
}

/**
 * A dropdown over [options]. [extra] adds trailing menu entries, e.g. "New series…", after a
 * divider.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun <T> Picker(
    label: String,
    options: List<T>,
    selected: T?,
    onSelect: (T) -> Unit,
    text: (T) -> String,
    modifier: Modifier = Modifier,
    supportingText: String? = null,
    extra: List<Pair<String, () -> Unit>> = emptyList(),
) {
    var expanded by remember { mutableStateOf(false) }
    ExposedDropdownMenuBox(expanded = expanded, onExpandedChange = { expanded = it }, modifier = modifier) {
        OutlinedTextField(
            value = selected?.let(text).orEmpty(),
            onValueChange = {},
            readOnly = true,
            label = { Text(label) },
            supportingText = supportingText?.let { { Text(it) } },
            trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded) },
            modifier = Modifier.fillMaxWidth().menuAnchor(ExposedDropdownMenuAnchorType.PrimaryNotEditable),
        )
        ExposedDropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
            for (option in options) {
                DropdownMenuItem(
                    text = { Text(text(option)) },
                    onClick = {
                        onSelect(option)
                        expanded = false
                    },
                )
            }
            if (extra.isNotEmpty() && options.isNotEmpty()) HorizontalDivider()
            for ((title, action) in extra) {
                DropdownMenuItem(
                    text = { Text(title) },
                    onClick = {
                        expanded = false
                        action()
                    },
                )
            }
        }
    }
}

/** A labelled value in a stats grid. */
@Composable
fun Stat(label: String, value: String, modifier: Modifier = Modifier, detail: String? = null) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.titleLarge)
        detail?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
