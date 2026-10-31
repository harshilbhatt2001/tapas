package dev.tapas.app

import dev.tapas.api.models.Aggregation
import dev.tapas.api.models.Observation
import java.time.LocalDate
import java.time.temporal.ChronoUnit
import kotlin.math.abs
import kotlin.math.roundToLong

/** Weeks from [start] to [due], fractional. */
fun weeksBetween(start: LocalDate, due: LocalDate): Double =
    ChronoUnit.DAYS.between(start, due) / 7.0

/**
 * The running total a `sum` goal targets when it adds [perWeek] every week from [start] to [due],
 * on top of [startValue].
 */
fun targetForWeeklyRate(perWeek: Double, startValue: Double, start: LocalDate, due: LocalDate): Double =
    startValue + perWeek * weeksBetween(start, due)

/** The inverse of [targetForWeeklyRate]; `null` when the span is empty. */
fun weeklyRateForTarget(target: Double, startValue: Double, start: LocalDate, due: LocalDate): Double? =
    weeksBetween(start, due).takeIf { it > 0 }?.let { (target - startValue) / it }

/**
 * A series' weekly values on the same scale as a goal's reference [path], which starts at the
 * path's first point.
 *
 * A level series is plotted as read. A sum series becomes a running total from the path's start:
 * each week's total lands at the end of that week, so the actual line starts at the path's first
 * value and climbs as weeks complete.
 */
fun actualOnPath(weeks: List<Observation>, aggregation: Aggregation, path: List<Observation>): List<Observation> {
    val origin = path.firstOrNull() ?: return if (aggregation == Aggregation.level) weeks else emptyList()
    val since = weeks.filter { !it.week.isBefore(origin.week) }
    return when (aggregation) {
        Aggregation.level -> since
        Aggregation.sum -> {
            var total = origin.value
            listOf(origin) + since.map {
                total += it.value
                Observation(week = it.week.plusWeeks(1), value = total)
            }
        }
    }
}

/** A number as people write it: at most one decimal, none when whole. */
fun Double.short(): String {
    val tenths = (this * 10).roundToLong()
    return if (tenths % 10 == 0L) "${tenths / 10}" else "%.1f".format(tenths / 10.0)
}

/** Days ahead or behind as a phrase; under half a day counts as on time. */
fun daysPhrase(daysBehind: Double): String {
    val days = abs(daysBehind).roundToLong()
    return when {
        days == 0L -> "on time"
        daysBehind > 0 -> "$days ${if (days == 1L) "day" else "days"} behind"
        else -> "$days ${if (days == 1L) "day" else "days"} ahead"
    }
}
