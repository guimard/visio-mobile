package io.visio.mobile.auth

import android.annotation.SuppressLint
import android.net.http.SslError
import android.os.Bundle
import android.util.Log
import android.webkit.CookieManager
import android.webkit.SslErrorHandler
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Toast
import androidx.activity.ComponentActivity

class OidcLoginActivity : ComponentActivity() {

    companion object {
        const val EXTRA_MEET_INSTANCE = "meet_instance"
        private const val TAG = "OidcLogin"
    }

    private var cookieExtracted = false

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val meetInstance = intent.getStringExtra(EXTRA_MEET_INSTANCE)
        if (meetInstance == null) {
            Log.e(TAG, "No meet_instance extra provided")
            setResult(RESULT_CANCELED)
            finish()
            return
        }

        val returnTo = "https://$meetInstance/"
        val authUrl = "https://$meetInstance/api/v1.0/authenticate/?returnTo=${
            java.net.URLEncoder.encode(returnTo, "UTF-8")
        }"

        Log.d(TAG, "Starting OIDC flow: $authUrl")

        val cookieManager = CookieManager.getInstance()
        cookieManager.setAcceptCookie(true)

        val webView = WebView(this)
        webView.settings.javaScriptEnabled = true
        webView.settings.domStorageEnabled = true

        webView.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest,
            ): Boolean {
                Log.d(TAG, "shouldOverrideUrlLoading: ${request.url}")
                return false
            }

            override fun onPageFinished(view: WebView, url: String) {
                super.onPageFinished(view, url)
                Log.d(TAG, "onPageFinished: $url")
                if (!cookieExtracted && url.startsWith(returnTo) && !url.contains("/api/v1.0/authenticate")) {
                    tryExtractSessionCookie(meetInstance)
                }
            }

            override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError) {
                Log.e(TAG, "SSL error for ${error.url}: ${error.primaryError}")
                handler.cancel()
                showErrorAndFinish("SSL certificate error for ${error.url?.substringBefore("/", error.url.toString()) ?: "server"}")
            }

            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                if (request.isForMainFrame) {
                    Log.e(TAG, "WebView error: ${error.description} (${error.errorCode})")
                    showErrorAndFinish("Connection error: ${error.description}")
                }
            }
        }

        setContentView(webView)
        webView.loadUrl(authUrl)
    }

    private fun showErrorAndFinish(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
        setResult(RESULT_CANCELED)
        finish()
    }

    private fun tryExtractSessionCookie(meetInstance: String) {
        val allCookies = CookieManager.getInstance().getCookie("https://$meetInstance")
        Log.d(TAG, "Cookies for $meetInstance: $allCookies")

        if (allCookies == null) {
            Log.w(TAG, "No cookies found")
            return
        }

        val sessionId = allCookies.split(";")
            .map { it.trim() }
            .firstOrNull { it.startsWith("sessionid=") }
            ?.substringAfter("sessionid=")

        if (sessionId != null) {
            Log.d(TAG, "Session cookie extracted successfully")
            cookieExtracted = true
            intent.putExtra("sessionid", sessionId)
            setResult(RESULT_OK, intent)
            finish()
        } else {
            Log.w(TAG, "sessionid not found in cookies")
        }
    }
}
