package dev.tapas.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.tapas.api.models.ConnectGoogle
import dev.tapas.api.models.Course
import dev.tapas.api.models.Goal
import dev.tapas.api.models.GoalInput
import dev.tapas.api.models.GoogleApi
import dev.tapas.api.models.GrantStatus
import dev.tapas.api.models.ManualEntry
import dev.tapas.api.models.Me
import dev.tapas.api.models.Observation
import dev.tapas.api.models.Series
import dev.tapas.api.models.SeriesInput
import dev.tapas.api.models.SignIn
import java.time.LocalDate
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

sealed interface Screen {
    data object Loading : Screen

    /** [continueAutomatically] is false once the user dismissed Google's sheet. */
    data class SignedOut(val continueAutomatically: Boolean) : Screen

    data class Home(val home: HomeData) : Screen
}

/**
 * Everything the signed-in screens show. [course] is `null` with [courseError] set when the server
 * could not judge the tree; goals and logs stay editable meanwhile.
 */
data class HomeData(
    val me: Me,
    val goals: List<Goal>,
    val series: List<Series>,
    val course: Course?,
    val courseError: String?,
)

data class UiState(
    val screen: Screen = Screen.Loading,
    val error: String? = null,
    val refreshing: Boolean = false,
)

class TapasViewModel(application: Application) : AndroidViewModel(application) {
    private val session = SessionStore(application)

    @Volatile
    private var sessionToken: String? = null
    private val api = tapasApi { sessionToken }

    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    /**
     * Everything Google in one go: sign in if there is no session, ask consent for any API the
     * server can't read yet, then load. Consent screens appear only the first time.
     */
    fun continueWithGoogle(google: GoogleAccount) {
        viewModelScope.launch {
            try {
                if (state.value.screen !is Screen.Home) show(Screen.Loading)
                if (sessionToken == null) sessionToken = session.token.first()
                if (sessionToken == null) signIn(google)
                val me = api.me().bodyOrThrow()
                for (missing in GoogleApi.entries.filter { it.needsConsent(me) }) {
                    api.connectGoogle(ConnectGoogle(missing, google.serverAuthCode(missing)))
                        .successOrThrow()
                }
                load()
            } catch (_: SignedOutException) {
                signedOut()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                val screen = state.value.screen
                _state.value = UiState(
                    if (screen is Screen.Home) screen else Screen.SignedOut(false),
                    error.message ?: error.toString(),
                )
            }
        }
    }

    /** Pull-to-refresh: reloads the tree, series and course in place. */
    fun refresh() {
        viewModelScope.launch {
            _state.update { it.copy(refreshing = true) }
            reload()
        }
    }

    fun dismissError() {
        _state.update { it.copy(error = null) }
    }

    fun signOut() {
        viewModelScope.launch {
            runCatching { api.signOut() }
            session.clear()
            sessionToken = null
            show(Screen.SignedOut(continueAutomatically = false))
        }
    }

    /** Creates the goal when [id] is `null`, else replaces it. */
    suspend fun saveGoal(id: Long?, input: GoalInput): Result<Goal> = edit {
        if (id == null) api.createGoal(input).bodyOrThrow() else api.updateGoal(id, input).bodyOrThrow()
    }

    /** Deletes the goal with its whole subtree. */
    suspend fun deleteGoal(id: Long): Result<Unit> = edit { api.deleteGoal(id).successOrThrow() }

    suspend fun createSeries(input: SeriesInput): Result<Series> =
        edit { api.createSeries(input).bodyOrThrow() }

    /** Stores [value] as the series' reading for [day], replacing any earlier one. */
    suspend fun logEntry(seriesId: Long, day: LocalDate, value: Double): Result<Unit> =
        edit { api.putManualEntry(seriesId, day, ManualEntry(value)).successOrThrow() }

    /** The series' weekly values, oldest first. */
    suspend fun seriesWeeks(seriesId: Long): Result<List<Observation>> =
        attempt { api.seriesWeeks(seriesId).bodyOrThrow() }

    private suspend fun signIn(google: GoogleAccount) {
        val token = try {
            api.signIn(SignIn(google.idToken())).bodyOrThrow().token
        } catch (_: SignedOutException) {
            error("This Google account may not use tapas")
        }
        session.save(token)
        sessionToken = token
    }

    /** Reloads until every connected API has finished its first sync on the server. */
    private suspend fun load() {
        while (true) {
            val home = fetchHome()
            show(Screen.Home(home))
            if (home.me.grants.none { it.firstSyncPending }) return
            delay(FIRST_SYNC_POLL)
        }
    }

    private suspend fun fetchHome(): HomeData = coroutineScope {
        val me = async { api.me().bodyOrThrow() }
        val goals = async { api.goals().bodyOrThrow() }
        val series = async { api.series().bodyOrThrow() }
        val course = async {
            try {
                Result.success(api.course().bodyOrThrow())
            } catch (gone: SignedOutException) {
                throw gone
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                Result.failure(error)
            }
        }
        HomeData(
            me = me.await(),
            goals = goals.await(),
            series = series.await(),
            course = course.await().getOrNull(),
            courseError = course.await().exceptionOrNull()?.message,
        )
    }

    /** Shows fresh data, or keeps the old data with the error on top. */
    private suspend fun reload() {
        attempt { fetchHome() }
            .onSuccess { show(Screen.Home(it)) }
            .onFailure { failure ->
                if (failure !is SignedOutException) {
                    _state.update { it.copy(error = failure.message, refreshing = false) }
                }
            }
    }

    /** [attempt], then a reload so every screen sees the change. */
    private suspend fun <T> edit(block: suspend () -> T): Result<T> =
        attempt(block).onSuccess { reload() }

    /** Runs one request; a lost session signs out, anything else comes back as a failure. */
    private suspend fun <T> attempt(block: suspend () -> T): Result<T> =
        try {
            Result.success(block())
        } catch (gone: SignedOutException) {
            signedOut()
            Result.failure(gone)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            Result.failure(error)
        }

    private suspend fun signedOut() {
        session.clear()
        sessionToken = null
        show(Screen.SignedOut(continueAutomatically = true))
    }

    private fun show(screen: Screen) {
        _state.update { UiState(screen) }
    }
}

private val FIRST_SYNC_POLL = 3.seconds

private val GrantStatus.firstSyncPending: Boolean
    get() = !needsConsent && syncedAt == null && syncError == null

private fun GoogleApi.needsConsent(me: Me): Boolean =
    me.grants.none { it.api == this && !it.needsConsent }
