package dev.tapas.app

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.expressiveLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext

/**
 * Material 3 Expressive with the wallpaper's colours (Material You) on Android 12+, the expressive
 * baseline scheme before that, and expressive spring motion throughout.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TapasTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    val colorScheme = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        dark -> darkColorScheme()
        else -> expressiveLightColorScheme()
    }
    MaterialExpressiveTheme(
        colorScheme = colorScheme,
        motionScheme = MotionScheme.expressive(),
        content = content,
    )
}

/** A container colour and the colour of content on it. */
data class ContainerColors(val container: Color, val content: Color)

/**
 * Amber for "needs attention but not broken" (behind, due soon). Material has no warning role, so
 * this fixed pair sits beside the scheme's error role for broken states.
 */
val ColorScheme.caution: ContainerColors
    get() = if (background.luminance() < 0.5f) {
        ContainerColors(Color(0xFF5C3F00), Color(0xFFFFDEA6))
    } else {
        ContainerColors(Color(0xFFFFDEA6), Color(0xFF261900))
    }
