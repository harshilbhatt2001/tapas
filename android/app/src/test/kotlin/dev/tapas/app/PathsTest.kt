package dev.tapas.app

import dev.tapas.api.models.Aggregation
import dev.tapas.api.models.Observation
import java.time.LocalDate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

private fun obs(week: LocalDate, value: Double) = Observation(week = week, value = value)

class PathsTest {
    private val monday = LocalDate.of(2026, 1, 5)

    @Test
    fun weeklyRateFillsTheRunningTotal() {
        assertEquals(60.0, targetForWeeklyRate(5.0, 0.0, monday, monday.plusWeeks(12)), 1e-9)
        assertEquals(70.0, targetForWeeklyRate(5.0, 10.0, monday, monday.plusWeeks(12)), 1e-9)
    }

    @Test
    fun targetGivesBackTheWeeklyRate() {
        assertEquals(5.0, weeklyRateForTarget(70.0, 10.0, monday, monday.plusWeeks(12))!!, 1e-9)
        assertNull(weeklyRateForTarget(70.0, 10.0, monday, monday))
    }

    @Test
    fun sumSeriesBecomesARunningTotalFromThePathStart() {
        val path = listOf(obs(monday, 0.0), obs(monday.plusWeeks(2), 10.0))
        val weeks = listOf(
            obs(monday.minusWeeks(1), 99.0),
            obs(monday, 4.0),
            obs(monday.plusWeeks(1), 6.0),
        )
        assertEquals(
            listOf(
                obs(monday, 0.0),
                obs(monday.plusWeeks(1), 4.0),
                obs(monday.plusWeeks(2), 10.0),
            ),
            actualOnPath(weeks, Aggregation.sum, path),
        )
    }

    @Test
    fun levelSeriesIsPlottedAsReadFromThePathStart() {
        val path = listOf(obs(monday, 80.0))
        val weeks = listOf(obs(monday.minusWeeks(1), 81.0), obs(monday, 80.5))
        assertEquals(listOf(obs(monday, 80.5)), actualOnPath(weeks, Aggregation.level, path))
    }

    @Test
    fun daysRoundToAPhrase() {
        assertEquals("on time", daysPhrase(0.4))
        assertEquals("1 day behind", daysPhrase(1.2))
        assertEquals("12 days ahead", daysPhrase(-12.0))
    }
}
