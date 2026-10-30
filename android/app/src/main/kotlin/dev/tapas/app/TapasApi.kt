package dev.tapas.app

import dev.tapas.api.apis.DefaultApi
import dev.tapas.api.infrastructure.ApiClient
import okhttp3.OkHttpClient
import retrofit2.Response

/** The generated client, sending the current session token as `Authorization: Bearer`. */
fun tapasApi(sessionToken: () -> String?): DefaultApi {
    val http = OkHttpClient.Builder().addInterceptor { chain ->
        val request = sessionToken()
            ?.let { chain.request().newBuilder().header("Authorization", "Bearer $it").build() }
            ?: chain.request()
        chain.proceed(request)
    }
    return ApiClient(BuildConfig.TAPAS_URL, http).createService(DefaultApi::class.java)
}

class SignedOutException : Exception("signed out")

class ApiException(response: Response<*>) :
    Exception("${response.code()}: ${response.errorBody()?.string().orEmpty()}")

/** The body of a successful response; a 401 means the session is gone. */
fun <T> Response<T>.bodyOrThrow(): T = when {
    code() == 401 -> throw SignedOutException()
    isSuccessful -> body() ?: throw ApiException(this)
    else -> throw ApiException(this)
}

fun Response<Unit>.successOrThrow() {
    if (code() == 401) throw SignedOutException()
    if (!isSuccessful) throw ApiException(this)
}
