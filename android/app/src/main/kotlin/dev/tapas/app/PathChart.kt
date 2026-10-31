package dev.tapas.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import dev.tapas.api.models.Observation
import java.time.LocalDate

/**
 * The reference path (dashed) against the actual series (solid, with a dot per week), with today
 * and the due date marked. Both lines share one value axis whose extremes are labelled.
 */
@Composable
fun PathChart(
    path: List<Observation>,
    actual: List<Observation>,
    unit: String,
    modifier: Modifier = Modifier,
    due: LocalDate? = null,
    today: LocalDate = LocalDate.now(),
) {
    val points = path + actual
    if (points.isEmpty()) return
    val scheme = MaterialTheme.colorScheme
    val labelStyle = MaterialTheme.typography.labelSmall.copy(color = scheme.onSurfaceVariant)
    val measurer = rememberTextMeasurer()

    val firstDay = points.minOf { it.week }.toEpochDay()
    val lastDay = maxOf(points.maxOf { it.week }.toEpochDay(), due?.toEpochDay() ?: Long.MIN_VALUE)
    val low = points.minOf { it.value }
    val high = points.maxOf { it.value }
    val span = (high - low).takeIf { it > 0 } ?: 1.0
    val description = buildString {
        append("Chart of the reference path against actual values.")
        path.lastOrNull()?.let { append(" Path ends at ${it.value.short()} $unit.") }
        actual.lastOrNull()?.let { append(" Latest actual ${it.value.short()} $unit.") }
    }

    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Canvas(Modifier.fillMaxWidth().height(200.dp).semantics { contentDescription = description }) {
            val left = 48.dp.toPx()
            val bottom = size.height - 20.dp.toPx()
            val top = 8.dp.toPx()
            fun x(day: LocalDate): Float {
                val days = (lastDay - firstDay).coerceAtLeast(1)
                return left + (size.width - left) * (day.toEpochDay() - firstDay) / days
            }
            fun y(value: Double): Float = (bottom - (bottom - top) * ((value - low) / span)).toFloat()

            for (value in listOf(low, high)) {
                drawLine(scheme.outlineVariant, Offset(left, y(value)), Offset(size.width, y(value)), 1.dp.toPx())
                val text = measurer.measure("${value.short()} $unit", labelStyle)
                drawText(text, topLeft = Offset(0f, y(value) - text.size.height / 2f))
            }
            if (today.toEpochDay() in firstDay..lastDay) marker(x(today), top, bottom, scheme.tertiary)
            due?.let { marker(x(it), top, bottom, scheme.error) }
            listOf(LocalDate.ofEpochDay(firstDay), LocalDate.ofEpochDay(lastDay)).forEachIndexed { i, day ->
                val text = measurer.measure(day.medium(), labelStyle)
                val at = if (i == 0) x(day) else x(day) - text.size.width
                drawText(text, topLeft = Offset(at, bottom + 4.dp.toPx()))
            }

            line(path.map { Offset(x(it.week), y(it.value)) }, scheme.outline, dashed = true)
            val actualPoints = actual.map { Offset(x(it.week), y(it.value)) }
            line(actualPoints, scheme.primary, dashed = false)
            actualPoints.forEach { drawCircle(scheme.primary, 4.dp.toPx(), it) }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Legend(scheme.outline, "Reference path")
            Legend(scheme.primary, "Actual")
            Legend(scheme.tertiary, "Today")
            if (due != null) Legend(scheme.error, "Due")
        }
    }
}

private fun DrawScope.line(points: List<Offset>, color: Color, dashed: Boolean) {
    if (points.size < 2) return
    val path = Path().apply {
        moveTo(points[0].x, points[0].y)
        points.drop(1).forEach { lineTo(it.x, it.y) }
    }
    drawPath(
        path,
        color,
        style = Stroke(
            width = 3.dp.toPx(),
            cap = StrokeCap.Round,
            join = StrokeJoin.Round,
            pathEffect = if (dashed) PathEffect.dashPathEffect(floatArrayOf(12f, 10f)) else null,
        ),
    )
}

private fun DrawScope.marker(x: Float, top: Float, bottom: Float, color: Color) {
    drawLine(
        color,
        Offset(x, top),
        Offset(x, bottom),
        2.dp.toPx(),
        pathEffect = PathEffect.dashPathEffect(floatArrayOf(4f, 6f)),
    )
}

@Composable
private fun Legend(color: Color, label: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Canvas(Modifier.size(12.dp)) { drawCircle(color) }
        Text(label, style = MaterialTheme.typography.labelSmall)
    }
}
