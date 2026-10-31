package dev.tapas.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AccountTree
import androidx.compose.material.icons.rounded.EditNote
import androidx.compose.material.icons.rounded.Insights
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ShortNavigationBar
import androidx.compose.material3.ShortNavigationBarItem
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.navigation3.runtime.NavBackStack
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.ui.NavDisplay
import kotlinx.serialization.Serializable

/** Where the app can be; the top three are the navigation bar's destinations. */
sealed interface Route : NavKey {
    @Serializable data object Course : Route

    @Serializable data object Goals : Route

    @Serializable data object Log : Route

    @Serializable data class Node(val goalId: Long) : Route

    /** A new goal when [goalId] is `null`, under [parentId] if given. */
    @Serializable data class GoalEditor(val goalId: Long? = null, val parentId: Long? = null) : Route

    @Serializable data object Account : Route
}

private enum class Destination(val route: Route, val label: String, val icon: ImageVector) {
    Course(Route.Course, "Course", Icons.Rounded.Insights),
    Goals(Route.Goals, "Goals", Icons.Rounded.AccountTree),
    Log(Route.Log, "Log", Icons.Rounded.EditNote),
}

/** The view model and the shell's navigation, provided to every screen through [LocalActions]. */
class Actions(
    val viewModel: TapasViewModel,
    val onContinueWithGoogle: () -> Unit,
    val navigate: (Route) -> Unit,
    val back: () -> Unit,
    /** Pops screens off the top while [predicate] holds for them, never the last one. */
    val popWhile: (predicate: (Route) -> Boolean) -> Unit,
)

val LocalActions = staticCompositionLocalOf<Actions> { error("no Actions provided") }

@Composable
fun TapasApp(state: UiState, viewModel: TapasViewModel, onContinueWithGoogle: () -> Unit) {
    when (val screen = state.screen) {
        Screen.Loading -> Box(Modifier.fillMaxSize(), Alignment.Center) { Loading() }
        is Screen.SignedOut -> SignedOut(screen, state.error, onContinueWithGoogle)
        is Screen.Home -> Home(state, screen.home, viewModel, onContinueWithGoogle)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun Loading(modifier: Modifier = Modifier) {
    LoadingIndicator(modifier.size(64.dp))
}

@Composable
private fun SignedOut(screen: Screen.SignedOut, error: String?, onContinueWithGoogle: () -> Unit) {
    if (screen.continueAutomatically) {
        LaunchedEffect(screen) { onContinueWithGoogle() }
    }
    Scaffold { padding ->
        Column(
            Modifier.padding(padding).padding(24.dp).fillMaxSize(),
            verticalArrangement = Arrangement.spacedBy(24.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("tapas", style = MaterialTheme.typography.displayMediumEmphasized)
            Text(
                "Steer every goal toward the target state.",
                style = MaterialTheme.typography.bodyLarge,
                textAlign = TextAlign.Center,
            )
            Button(onClick = onContinueWithGoogle) { Text("Continue with Google") }
            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center)
            }
        }
    }
}

/**
 * The signed-in shell: a short navigation bar over the three destinations, hidden on detail and
 * editor screens. Switching destination resets the stack to Course plus that destination, so back
 * from Goals or Log lands on Course.
 */
@Composable
private fun Home(state: UiState, home: HomeData, viewModel: TapasViewModel, onContinueWithGoogle: () -> Unit) {
    val backStack = rememberNavBackStack(Route.Course)
    val top = backStack.lastOrNull()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(state.error) {
        state.error?.let {
            snackbar.showSnackbar(it)
            viewModel.dismissError()
        }
    }
    val actions = remember(viewModel, backStack) {
        Actions(
            viewModel = viewModel,
            onContinueWithGoogle = onContinueWithGoogle,
            navigate = { backStack.add(it) },
            back = { if (backStack.size > 1) backStack.removeAt(backStack.lastIndex) },
            popWhile = { predicate ->
                while (backStack.size > 1 && predicate(backStack.last() as Route)) {
                    backStack.removeAt(backStack.lastIndex)
                }
            },
        )
    }
    CompositionLocalProvider(LocalActions provides actions) {
        Scaffold(
            contentWindowInsets = WindowInsets(0),
            snackbarHost = { SnackbarHost(snackbar) },
            bottomBar = {
                if (Destination.entries.any { it.route == top }) {
                    ShortNavigationBar {
                        for (destination in Destination.entries) {
                            ShortNavigationBarItem(
                                selected = destination.route == top,
                                onClick = { backStack.switchTo(destination.route) },
                                icon = { Icon(destination.icon, contentDescription = null) },
                                label = { Text(destination.label) },
                            )
                        }
                    }
                }
            },
        ) { padding ->
            NavDisplay(
                backStack = backStack,
                modifier = Modifier.padding(padding).consumeWindowInsets(padding),
                entryProvider = entryProvider {
                    entry<Route.Course> { CourseScreen(home, state.refreshing) }
                    entry<Route.Goals> { GoalsScreen(home) }
                    entry<Route.Log> { LogScreen(home) }
                    entry<Route.Node> { NodeScreen(home, it.goalId) }
                    entry<Route.GoalEditor> { GoalEditorScreen(home, it.goalId, it.parentId) }
                    entry<Route.Account> { AccountScreen(home.me) }
                },
            )
        }
    }
}

private fun NavBackStack<NavKey>.switchTo(route: Route) {
    if (lastOrNull() == route) return
    while (size > 1) removeAt(lastIndex)
    if (first() != Route.Course) set(0, Route.Course)
    if (route != Route.Course) add(route)
}
