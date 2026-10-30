package dev.tapas.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.google.android.gms.auth.api.identity.AuthorizationResult
import com.google.android.gms.auth.api.identity.Identity
import dev.tapas.api.models.GoogleApi
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val viewModel: TapasViewModel by viewModels()

    private val consent = registerForActivityResult(
        ActivityResultContracts.StartIntentSenderForResult(),
    ) { result ->
        val api = viewModel.apiAwaitingConsent ?: return@registerForActivityResult
        viewModel.apiAwaitingConsent = null
        runCatching {
            Identity.getAuthorizationClient(this).getAuthorizationResultFromIntent(result.data)
        }.fold({ sendAuthCode(api, it) }, viewModel::fail)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val state by viewModel.state.collectAsStateWithLifecycle()
            MaterialTheme {
                TapasApp(
                    state = state,
                    onSignIn = ::signIn,
                    onSignOut = viewModel::signOut,
                    onRefresh = viewModel::refresh,
                    onConnect = ::connect,
                )
            }
        }
    }

    private fun signIn() {
        lifecycleScope.launch {
            try {
                viewModel.signIn(googleIdToken(this@MainActivity))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                viewModel.fail(error)
            }
        }
    }

    private fun connect(api: GoogleApi) {
        Identity.getAuthorizationClient(this)
            .authorize(authorizationRequest(api))
            .addOnSuccessListener { result ->
                val pendingIntent = result.pendingIntent
                if (result.hasResolution() && pendingIntent != null) {
                    viewModel.apiAwaitingConsent = api
                    consent.launch(IntentSenderRequest.Builder(pendingIntent.intentSender).build())
                } else {
                    sendAuthCode(api, result)
                }
            }
            .addOnFailureListener(viewModel::fail)
    }

    private fun sendAuthCode(api: GoogleApi, result: AuthorizationResult) {
        val code = result.serverAuthCode
        if (code == null) {
            viewModel.fail(IllegalStateException("Google returned no server auth code"))
        } else {
            viewModel.connect(api, code)
        }
    }
}
