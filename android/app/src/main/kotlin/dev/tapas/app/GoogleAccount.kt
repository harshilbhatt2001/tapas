package dev.tapas.app

import android.app.Activity
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.common.api.Scope
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import dev.tapas.api.models.GoogleApi

/** A Google ID token for the server's Web client, from the account the user picks. */
suspend fun googleIdToken(activity: Activity): String {
    val option = GetSignInWithGoogleOption.Builder(BuildConfig.GOOGLE_WEB_CLIENT_ID).build()
    val response = CredentialManager.create(activity)
        .getCredential(activity, GetCredentialRequest(listOf(option)))
    val credential = response.credential
    check(
        credential is CustomCredential &&
            credential.type == GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL,
    ) { "unexpected credential type ${credential.type}" }
    return GoogleIdTokenCredential.createFrom(credential.data).idToken
}

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

/**
 * Consent for one API alone, since Health rejects tokens carrying other APIs' scopes. Offline
 * access yields a server auth code that the server exchanges for its own refresh token.
 */
fun authorizationRequest(api: GoogleApi): AuthorizationRequest =
    AuthorizationRequest.builder()
        .setRequestedScopes(api.scopes.map(::Scope))
        .requestOfflineAccess(BuildConfig.GOOGLE_WEB_CLIENT_ID, true)
        .build()
