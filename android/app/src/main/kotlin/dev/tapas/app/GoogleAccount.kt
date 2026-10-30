package dev.tapas.app

import android.content.Intent
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.exceptions.NoCredentialException
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.common.api.Scope
import com.google.android.libraries.identity.googleid.GetGoogleIdOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import dev.tapas.api.models.GoogleApi
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.tasks.await

/** Must match `GoogleApi::scopes` in crates/sources/src/google/oauth.rs. */
val GoogleApi.scopes: List<String>
    get() = when (this) {
        GoogleApi.health -> listOf(
            "https://www.googleapis.com/auth/googlehealth.activity_and_fitness.readonly",
            "https://www.googleapis.com/auth/googlehealth.health_metrics_and_measurements.readonly",
        )
        GoogleApi.calendar -> listOf(
            "https://www.googleapis.com/auth/calendar.app.created",
            "https://www.googleapis.com/auth/calendar.freebusy",
        )
    }

/** The Google account the app signs in with. Register it in the activity's `onCreate`. */
class GoogleAccount(private val activity: ComponentActivity) {
    private var pendingConsent: CompletableDeferred<ActivityResult>? = null
    private val consent = activity.registerForActivityResult(
        ActivityResultContracts.StartIntentSenderForResult(),
    ) { pendingConsent?.complete(it) }

    /**
     * A Google ID token for the server's Web client. A returning user is signed in without a
     * tap; otherwise the account sheet slides up.
     */
    suspend fun idToken(): String =
        try {
            idToken(returningUser = true)
        } catch (_: NoCredentialException) {
            idToken(returningUser = false)
        }

    private suspend fun idToken(returningUser: Boolean): String {
        val option = GetGoogleIdOption.Builder()
            .setServerClientId(BuildConfig.GOOGLE_WEB_CLIENT_ID)
            .setFilterByAuthorizedAccounts(returningUser)
            .setAutoSelectEnabled(returningUser)
            .build()
        val credential = CredentialManager.create(activity)
            .getCredential(activity, GetCredentialRequest(listOf(option)))
            .credential
        check(
            credential is CustomCredential &&
                credential.type == GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL,
        ) { "unexpected credential type ${credential.type}" }
        return GoogleIdTokenCredential.createFrom(credential.data).idToken
    }

    /**
     * A server auth code for one API, showing Google's consent screen if the user hasn't agreed
     * yet. One API per consent, since Health rejects tokens carrying other APIs' scopes.
     */
    suspend fun serverAuthCode(api: GoogleApi): String {
        val client = Identity.getAuthorizationClient(activity)
        val request = AuthorizationRequest.builder()
            .setRequestedScopes(api.scopes.map(::Scope))
            .requestOfflineAccess(BuildConfig.GOOGLE_WEB_CLIENT_ID, true)
            .build()
        var result = client.authorize(request).await()
        val pendingIntent = result.pendingIntent
        if (result.hasResolution() && pendingIntent != null) {
            val answer = CompletableDeferred<ActivityResult>().also { pendingConsent = it }
            consent.launch(IntentSenderRequest.Builder(pendingIntent.intentSender).build())
            result = client.getAuthorizationResultFromIntent(answer.await().data ?: Intent())
        }
        return checkNotNull(result.serverAuthCode) { "Google returned no server auth code" }
    }
}
