package dev.tapas.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.tapas.api.models.ConnectGoogle
import dev.tapas.api.models.GoogleApi
import dev.tapas.api.models.Me
import dev.tapas.api.models.Recent
import dev.tapas.api.models.SignIn
import kotlinx.coroutines.CancellationException
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

    data class Home(val me: Me, val recent: Recent) : Screen
}

data class UiState(val screen: Screen = Screen.Loading, val error: String? = null)

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
                session.clear()
                sessionToken = null
                show(Screen.SignedOut(continueAutomatically = true))
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

    fun signOut() {
        viewModelScope.launch {
            runCatching { api.signOut() }
            session.clear()
            sessionToken = null
            show(Screen.SignedOut(continueAutomatically = false))
        }
    }

    private suspend fun signIn(google: GoogleAccount) {
        val token = try {
            api.signIn(SignIn(google.idToken())).bodyOrThrow().token
        } catch (_: SignedOutException) {
            error("This Google account may not use tapas")
        }
        session.save(token)
        sessionToken = token
    }

    private suspend fun load() {
        show(Screen.Home(api.me().bodyOrThrow(), api.recent().bodyOrThrow()))
    }

    private fun show(screen: Screen) {
        _state.update { UiState(screen) }
    }
}

private fun GoogleApi.needsConsent(me: Me): Boolean =
    me.grants.none { it.api == this && !it.needsConsent }
