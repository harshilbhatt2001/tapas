package dev.tapas.app

import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalKind
import dev.tapas.api.models.NodeState
import java.time.LocalDate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class GoalTreeTest {
    private fun goal(id: Long, parent: Long?, due: LocalDate? = null, kind: GoalKind = GoalKind.controlled) =
        Goal(id = id, kind = kind, title = "g$id", weight = 1.0, parentId = parent, due = due)

    private val day = LocalDate.of(2026, 6, 1)
    private val goals = listOf(
        goal(3, 1, day.plusDays(30)),
        goal(2, 1, day),
        goal(1, null, kind = GoalKind.master),
        goal(5, 2),
        goal(4, 2, day),
    )

    @Test
    fun rowsAreDepthFirstWithSiblingsByDueDate() {
        assertEquals(
            listOf(1L to 0, 2L to 1, 4L to 2, 5L to 2, 3L to 1),
            goals.treeRows().map { it.goal.id to it.depth },
        )
    }

    @Test
    fun collapsedGoalsHideTheirCheckpoints() {
        assertEquals(listOf(1L, 2L, 3L), goals.treeRows(setOf(2)).map { it.goal.id })
    }

    @Test
    fun subtreeHoldsEveryDescendant() {
        assertEquals(setOf(2L, 4L, 5L), goals.subtree(2))
    }

    @Test
    fun activeCheckpointIsTheFirstUnmerged() {
        val stack = goals.checkpoints(1).zip(listOf(NodeState.merged, NodeState.behind))
        assertEquals(3L, stack.activeCheckpoint()?.id)
    }

    @Test
    fun draftSendsRampAsAFraction() {
        val input = GoalDraft(
            kind = GoalKind.controlled,
            parentId = 1,
            title = "Deep work",
            seriesId = 7,
            targetValue = "60",
            maxRampPercent = "10",
            weight = "0,5",
        ).toInput()
        assertEquals(0.1, input.maxRamp!!, 1e-9)
        assertEquals(0.5, input.weight, 1e-9)
    }

    @Test
    fun draftRejectsAControlledGoalWithoutTarget() {
        val draft = GoalDraft(kind = GoalKind.controlled, parentId = 1, title = "x", seriesId = 7)
        assertThrows(IllegalArgumentException::class.java) { draft.toInput() }
    }
}
