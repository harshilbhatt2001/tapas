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
    data object SignedOut : Screen
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

    /** The API whose consent screen is showing, so its result can be sent to the server. */
    var apiAwaitingConsent: GoogleApi? = null

    init {
        viewModelScope.launch {
            sessionToken = session.token.first()
            if (sessionToken == null) show(Screen.SignedOut) else refresh()
        }
    }

    fun signIn(idToken: String) = launchAction {
        val token = api.signIn(SignIn(idToken)).bodyOrThrow().token
        session.save(token)
        sessionToken = token
        load()
    }

    fun signOut() = launchAction {
        api.signOut()
        forgetSession()
    }

    fun refresh() = launchAction { load() }

    fun connect(api: GoogleApi, serverAuthCode: String) = launchAction {
        this.api.connectGoogle(ConnectGoogle(api, serverAuthCode)).successOrThrow()
        load()
    }

    fun fail(error: Throwable) {
        _state.update { it.copy(error = error.message ?: error.toString()) }
    }

    private suspend fun load() {
        show(Screen.Home(api.me().bodyOrThrow(), api.recent().bodyOrThrow()))
    }

    private suspend fun forgetSession() {
        session.clear()
        sessionToken = null
        show(Screen.SignedOut)
    }

    private fun show(screen: Screen) {
        _state.value = UiState(screen)
    }

    private fun launchAction(action: suspend () -> Unit) {
        viewModelScope.launch {
            try {
                action()
            } catch (_: SignedOutException) {
                forgetSession()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                fail(error)
            }
        }
    }
}
