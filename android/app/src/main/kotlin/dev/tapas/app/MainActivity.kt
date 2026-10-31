package dev.tapas.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle

class MainActivity : ComponentActivity() {
    private val viewModel: TapasViewModel by viewModels()
    private val google = GoogleAccount(this)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) viewModel.continueWithGoogle(google)
        setContent {
            val state by viewModel.state.collectAsStateWithLifecycle()
            TapasTheme {
                TapasApp(
                    state = state,
                    viewModel = viewModel,
                    onContinueWithGoogle = { viewModel.continueWithGoogle(google) },
                )
            }
        }
    }
}
