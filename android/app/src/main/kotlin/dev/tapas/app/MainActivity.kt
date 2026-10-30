package dev.tapas.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle

class MainActivity : ComponentActivity() {
    private val viewModel: TapasViewModel by viewModels()
    private val google = GoogleAccount(this)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) viewModel.continueWithGoogle(google)
        setContent {
            val state by viewModel.state.collectAsStateWithLifecycle()
            MaterialTheme {
                TapasApp(
                    state = state,
                    onContinueWithGoogle = { viewModel.continueWithGoogle(google) },
                    onSignOut = viewModel::signOut,
                )
            }
        }
    }
}
