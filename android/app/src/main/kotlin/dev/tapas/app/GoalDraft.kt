package dev.tapas.app

import androidx.compose.runtime.saveable.Saver
import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalInput
import dev.tapas.api.models.GoalKind
import java.math.BigDecimal
import java.math.RoundingMode
import java.time.LocalDate
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json

private object IsoDate : KSerializer<LocalDate> {
    override val descriptor = PrimitiveSerialDescriptor("IsoDate", PrimitiveKind.STRING)
    override fun serialize(encoder: Encoder, value: LocalDate) = encoder.encodeString(value.toString())
    override fun deserialize(decoder: Decoder): LocalDate = LocalDate.parse(decoder.decodeString())
}

/**
 * The goal editor's fields as typed: numbers stay text until saving, so a half-typed "1." is not
 * lost. [maxRampPercent] is in percent where the API wants a fraction.
 */
@Serializable
data class GoalDraft(
    val kind: GoalKind,
    val parentId: Long? = null,
    val title: String = "",
    @Serializable(IsoDate::class) val start: LocalDate? = null,
    @Serializable(IsoDate::class) val due: LocalDate? = null,
    val seriesId: Long? = null,
    val startValue: String = "",
    val targetValue: String = "",
    @Serializable(IsoDate::class) val doneOn: LocalDate? = null,
    val weight: String = "1",
    val weeklyHours: String = "",
    val maxRampPercent: String = "",
    val capacityHours: String = "",
) {
    /**
     * The request body, or [IllegalArgumentException] naming the first field that doesn't parse.
     * Fields that don't belong to [kind] are sent empty.
     */
    fun toInput(): GoalInput {
        require(title.isNotBlank()) { "Give the goal a title" }
        val master = kind == GoalKind.master
        val controlled = kind == GoalKind.controlled
        if (!master) requireNotNull(parentId) { "Pick a parent goal" }
        if (controlled) requireNotNull(seriesId) { "Pick the series this goal steers" }
        return GoalInput(
            title = title.trim(),
            kind = kind,
            weight = number(weight, "Weight") ?: 1.0,
            parentId = parentId.takeUnless { master },
            start = start.takeUnless { master },
            due = due.takeUnless { master },
            seriesId = seriesId.takeIf { controlled },
            startValue = if (controlled) number(startValue, "Start value") else null,
            targetValue = if (controlled) {
                requireNotNull(number(targetValue, "Target value")) { "Set a target value" }
            } else {
                null
            },
            doneOn = doneOn.takeIf { kind == GoalKind.milestone },
            weeklyHours = if (master) null else number(weeklyHours, "Weekly hours"),
            maxRamp = if (controlled) number(maxRampPercent, "Max ramp")?.div(100) else null,
            capacityHours = if (master) number(capacityHours, "Capacity hours") else null,
        )
    }

    companion object {
        fun of(goal: Goal) = GoalDraft(
            kind = goal.kind,
            parentId = goal.parentId,
            title = goal.title,
            start = goal.start,
            due = goal.due,
            seriesId = goal.seriesId,
            startValue = goal.startValue?.plain().orEmpty(),
            targetValue = goal.targetValue?.plain().orEmpty(),
            doneOn = goal.doneOn,
            weight = goal.weight.plain(),
            weeklyHours = goal.weeklyHours?.plain().orEmpty(),
            maxRampPercent = goal.maxRamp?.let { (it * 100).plain() }.orEmpty(),
            capacityHours = goal.capacityHours?.plain().orEmpty(),
        )

        val saver = Saver<GoalDraft, String>(
            save = { Json.encodeToString(serializer(), it) },
            restore = { Json.decodeFromString(serializer(), it) },
        )
    }
}

/** A number field: blank is `null`, a comma counts as the decimal point. */
fun number(text: String, field: String): Double? {
    if (text.isBlank()) return null
    return requireNotNull(text.trim().replace(',', '.').toDoubleOrNull()) { "$field is not a number" }
}

/** A number for a text field to edit: no float noise, no trailing zeros, no locale grouping. */
fun Double.plain(): String =
    BigDecimal(this).setScale(6, RoundingMode.HALF_UP).stripTrailingZeros().toPlainString()
