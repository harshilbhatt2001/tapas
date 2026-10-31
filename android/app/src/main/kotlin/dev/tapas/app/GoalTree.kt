package dev.tapas.app

import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalKind
import dev.tapas.api.models.NodeState

/** A goal placed in the tree; [depth] 0 is a root. */
data class TreeRow(val goal: Goal, val depth: Int, val hasChildren: Boolean)

/** Siblings stack like checkpoints: by due date, undated last. */
private val byDue = compareBy<Goal, java.time.LocalDate?>(nullsLast()) { it.due }.thenBy { it.id }

fun List<Goal>.master(): Goal? = firstOrNull { it.kind == GoalKind.master }

/** [parent]'s children in stack order. */
fun List<Goal>.checkpoints(parent: Long): List<Goal> = filter { it.parentId == parent }.sortedWith(byDue)

/**
 * The tree depth first, in display order, skipping the children of [collapsed] goals. Goals whose
 * parent is missing are shown as roots rather than lost.
 */
fun List<Goal>.treeRows(collapsed: Set<Long> = emptySet()): List<TreeRow> {
    val ids = mapTo(HashSet()) { it.id }
    val children = groupBy { it.parentId?.takeIf(ids::contains) }
    val rows = mutableListOf<TreeRow>()
    fun visit(goal: Goal, depth: Int) {
        val kids = children[goal.id].orEmpty()
        rows += TreeRow(goal, depth, kids.isNotEmpty())
        if (goal.id !in collapsed) kids.sortedWith(byDue).forEach { visit(it, depth + 1) }
    }
    children[null].orEmpty()
        .sortedWith(compareBy<Goal> { it.kind != GoalKind.master }.then(byDue))
        .forEach { visit(it, 0) }
    return rows
}

/** [id] and everything below it: the goals that cannot become its parent. */
fun List<Goal>.subtree(id: Long): Set<Long> {
    val result = mutableSetOf(id)
    var grew = true
    while (grew) grew = filter { it.parentId in result }.any { result.add(it.id) }
    return result
}

/** The checkpoint that is steering now: the first in the stack that has not merged or finished. */
fun List<Pair<Goal, NodeState?>>.activeCheckpoint(): Goal? =
    firstOrNull { (_, state) -> state != NodeState.merged && state != NodeState.done }?.first
