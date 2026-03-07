package io.visio.mobile.auth

import android.content.Context
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

class OidcAuthManager(context: Context) {
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()

    private val prefs = EncryptedSharedPreferences.create(
        context,
        "visio_auth",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    fun launchOidcFlow(context: android.app.Activity, meetInstance: String) {
        val authUrl = "https://$meetInstance/authenticate/?returnTo=https://$meetInstance/"
        val intent = CustomTabsIntent.Builder().build()
        intent.launchUrl(context, Uri.parse(authUrl))
    }

    fun saveCookie(cookie: String) {
        prefs.edit().putString("sessionid", cookie).apply()
    }

    fun getSavedCookie(): String? {
        return prefs.getString("sessionid", null)
    }

    fun clearCookie() {
        prefs.edit().remove("sessionid").apply()
    }
}
