package dev.tapas.app

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.sessionDataStore by preferencesDataStore(name = "session")
private val sessionTokenKey = stringPreferencesKey("token")

/** The server's session token, kept across app restarts. */
class SessionStore(private val context: Context) {
    val token: Flow<String?> = context.sessionDataStore.data.map { it[sessionTokenKey] }

    suspend fun save(token: String) {
        context.sessionDataStore.edit { it[sessionTokenKey] = token }
    }

    suspend fun clear() {
        context.sessionDataStore.edit { it.remove(sessionTokenKey) }
    }
}
