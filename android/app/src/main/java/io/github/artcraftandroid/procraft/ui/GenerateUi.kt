package io.github.artcraftandroid.procraft.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.artcraftandroid.procraft.canvas.BarStatus
import io.github.artcraftandroid.procraft.canvas.CanvasViewModel
import io.github.artcraftandroid.procraft.canvas.Connection
import io.github.artcraftandroid.procraft.canvas.Generate
import io.github.artcraftandroid.procraft.canvas.Panel
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

/** Generate's two glyphs, in the bar's 44 frame: a sparkle (the button, the main tab) and a gear (its settings). */
object GenerateIcons {
    val Sparkle: ImageVector by lazy {
        glyph(
            "generate-sparkle",
            PathFillType.NonZero,
            "M20 11C20.9 18.2 23.8 21.1 31 22C23.8 22.9 20.9 25.8 20 33C19.1 25.8 16.2 22.9 9 22C16.2 21.1 19.1 18.2 20 11Z" +
                "M30 9C30.4 11.9 31.6 13.1 34.5 13.5C31.6 13.9 30.4 15.1 30 18C29.6 15.1 28.4 13.9 25.5 13.5C28.4 13.1 29.6 11.9 30 9Z",
        )
    }
    val Gear: ImageVector by lazy { glyph("generate-gear", PathFillType.EvenOdd, gearPath()) }

    private fun glyph(name: String, fill: PathFillType, d: String): ImageVector =
        ImageVector.Builder(name = name, defaultWidth = 44.dp, defaultHeight = 44.dp, viewportWidth = 44f, viewportHeight = 44f)
            .addPath(pathData = addPathNodes(d), pathFillType = fill, fill = SolidColor(Color.White))
            .build()

    /** Eight teeth round a hole, centred in the frame. */
    private fun gearPath(): String {
        val (cx, cy, outer, inner, teeth) = listOf(22f, 22f, 15f, 11.6f, 8f)
        val b = StringBuilder()
        for (i in 0 until teeth.toInt()) {
            val a = 2.0 * Math.PI * i / teeth
            val w = Math.PI / teeth
            for ((k, at) in listOf(-0.62 to inner, -0.38 to outer, 0.38 to outer, 0.62 to inner).withIndex()) {
                val ang = a + w * at.first
                val x = cx + at.second * Math.cos(ang)
                val y = cy + at.second * Math.sin(ang)
                b.append(if (i == 0 && k == 0) "M" else "L").append(String.format(java.util.Locale.ROOT, "%.2f %.2f", x, y))
            }
        }
        val h = 5.2f
        b.append("ZM${cx + h} ${cy}A$h $h 0 1 0 ${cx - h} ${cy}A$h $h 0 1 0 ${cx + h} ${cy}Z")
        return b.toString()
    }
}

private val RingTrack get() = if (Pc.light) Color(0x30000000) else Color(0x40FFFFFF)
private val RingInk get() = if (Pc.light) Pc.Accent else Color.White
private val DoneGreen = Color(0xFF34C759)
private val FailedRed = Color(0xFFFF453A)
private val ConnectingOrange = Color(0xFFFF9F0A)
private val Idle get() = if (Pc.light) Color(0xFF404040) else Color(0xFFA3A3A3)
private val Label get() = if (Pc.light) Color(0xFF555555) else Color(0xFFAAAAAA)
private val FieldFill get() = if (Pc.light) Color(0xFFFFFFFF) else Color(0xFF0B0B0B)
private val BadgeFill get() = if (Pc.light) Color(0xF0F2F2F2) else Color(0xE6141414)

/** The bar status, read again each frame while something moves (the outcome fades after two seconds). */
@Composable
private fun rememberBarStatus(g: Generate): BarStatus {
    var now by remember { mutableLongStateOf(android.os.SystemClock.uptimeMillis()) }
    val status = g.barStatus(now)
    LaunchedEffect(status) {
        while (status != BarStatus.Idle) {
            delay(200)
            now = android.os.SystemClock.uptimeMillis()
        }
    }
    return status
}

/**
 * The bar's Generate button: its sparkle on a dark disc (blue while its panel is open). Tapped it
 * makes the last prompt again (or opens the panel with nothing set yet); held, it opens the panel.
 * While a job runs its ring fills with how far it has come and the sparkle pulses; as it ends the
 * ring shows green or red for a moment.
 */
@Composable
fun GenerateButton(vm: CanvasViewModel, width: Dp) {
    val status = rememberBarStatus(vm.generate)
    val lit = animateHighlight(vm.panel == Panel.GENERATE)
    val tap by rememberUpdatedState { vm.generatePressed() }
    val hold by rememberUpdatedState { vm.toggle(Panel.GENERATE) }
    val pulse = if (status is BarStatus.Working) {
        rememberInfiniteTransition(label = "pulse").animateFloat(0f, 1f, infiniteRepeatable(tween(800), RepeatMode.Reverse), label = "pulse").value
    } else {
        0f
    }
    val spin = if (status is BarStatus.Working && status.progress == null) {
        rememberInfiniteTransition(label = "spin").animateFloat(0f, 360f, infiniteRepeatable(tween(1000, easing = LinearEasing)), label = "spin").value
    } else {
        0f
    }
    Box(
        Modifier.size(width, Pc.BarButtonHeight).pointerInput(Unit) { detectTapGestures(onLongPress = { hold() }, onTap = { tap() }) },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(Pc.ActionDiscSize).clip(CircleShape).background(lerp(Pc.ActionDisc, Pc.BarGlyphSelected, lit)))
        Icon(GenerateIcons.Sparkle, contentDescription = "Generate", tint = lerp(Pc.ActionGlyph, Color.White, maxOf(lit, pulse)), modifier = Modifier.requiredSize(36.dp))
        if (status != BarStatus.Idle) {
            Box(
                Modifier.requiredSize(Pc.ActionDiscSize + 7.dp).drawBehind {
                    val stroke = Stroke(2.5.dp.toPx())
                    val inset = stroke.width / 2f
                    val box = Size(size.width - stroke.width, size.height - stroke.width)
                    when (status) {
                        is BarStatus.Working -> {
                            drawArc(RingTrack, 0f, 360f, false, Offset(inset, inset), box, style = stroke)
                            val (from, sweep) = status.progress?.let { -90f to 360f * it.coerceIn(0f, 1f) } ?: (spin - 90f to 100f)
                            drawArc(RingInk, from, sweep, false, Offset(inset, inset), box, style = stroke)
                        }
                        BarStatus.Done -> drawArc(DoneGreen, 0f, 360f, false, Offset(inset, inset), box, style = stroke)
                        BarStatus.Failed -> drawArc(FailedRed, 0f, 360f, false, Offset(inset, inset), box, style = stroke)
                        BarStatus.Idle -> Unit
                    }
                },
            )
        }
    }
}

/** While the panel is closed, a badge under the Generate button: Queued, the share done, or how it ended. */
@Composable
fun GenerateBadge(vm: CanvasViewModel, buttonCentreX: Dp) {
    if (vm.panel == Panel.GENERATE) return
    val status = rememberBarStatus(vm.generate)
    val (words, edge) = when (status) {
        BarStatus.Idle -> return
        is BarStatus.Working -> (status.progress?.let { "${(it * 100).roundToInt()}%" } ?: "Queued") to null
        BarStatus.Done -> "Done" to DoneGreen
        BarStatus.Failed -> "Failed" to FailedRed
    }
    val shape = RoundedCornerShape(11.dp)
    Box(
        Modifier.offset(x = buttonCentreX - 32.dp, y = Pc.BarHeight + 6.dp).size(64.dp, 22.dp).clip(shape).background(BadgeFill)
            .then(if (edge != null) Modifier.border(1.5.dp, edge, shape) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Text(words, color = Pc.Text, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

/** The Generate panel: what to make (the main tab), and the server and model (its settings). */
@Composable
fun GeneratePanel(vm: CanvasViewModel, arrowX: Dp, modifier: Modifier = Modifier) {
    val g = vm.generate
    var settings by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { g.connectOnce() }
    androidx.activity.compose.BackHandler(enabled = settings) { settings = false }
    Popover(PANEL_WIDTH, arrowX, modifier) {
        Row(Modifier.fillMaxWidth().height(60.dp).padding(start = 24.dp, top = 18.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(if (settings) "Settings" else "Generate", color = Pc.PanelTitle, fontSize = 24.sp, fontWeight = FontWeight.Bold, maxLines = 1)
            Box(Modifier.weight(1f))
            TabIcon(GenerateIcons.Sparkle, "Generate", !settings, 30.dp) { settings = false }
            TabIcon(GenerateIcons.Gear, "Settings", settings, 34.dp) { settings = true }
        }
        Column(
            Modifier.verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (settings) SettingsPage(g) else MainPage(vm, g) { settings = true }
        }
    }
    // what was typed is kept as the panel goes
    androidx.compose.runtime.DisposableEffect(Unit) { onDispose { g.keep() } }
}

private val PANEL_WIDTH = 400.dp

@Composable
private fun TabIcon(icon: ImageVector, label: String, chosen: Boolean, size: Dp, onTap: () -> Unit) {
    Box(Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onTap), contentAlignment = Alignment.Center) {
        Icon(icon, contentDescription = label, tint = if (chosen) Pc.Accent else Idle, modifier = Modifier.requiredSize(size))
    }
}

/** The main page: the model and how the server stands (a tap away from their settings), the prompt, the strength, Generate. */
@Composable
private fun MainPage(vm: CanvasViewModel, g: Generate, toSettings: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Pc.ActionsGroup).clickable(onClick = toSettings).padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val model = g.workflow.removeSuffix(".json").ifEmpty { "No model chosen" }
        Text(model, color = Pc.Text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        val (dot, words) = connectionLine(g)
        Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        Text(words, color = Label, fontSize = 13.sp, maxLines = 1, modifier = Modifier.padding(start = 6.dp))
    }
    FieldLabel("Prompt")
    Field(g.prompt, "What to make of the sketch", lines = 5) { g.prompt = it }
    if (g.chosen?.takesImage != false) {
        StrengthSlider(g.strength) { g.strength = it }
    }
    SeedRow(g)
    Spacer(Modifier.height(4.dp))
    if (g.busy) {
        val words = g.progress?.let { "Cancel  ·  ${(it * 100).roundToInt()}%" } ?: "Cancel"
        WideButton(words, enabled = true) { g.cancel() }
        ProgressLine(g.progress)
    } else {
        WideButton("Generate", enabled = g.workflow.isNotEmpty() && vm.session != null) { g.start() }
    }
    if (g.status.isNotEmpty()) Text(g.status, color = Label, fontSize = 14.sp)
}

/** The settings page: the server, the model, the model's negative prompt. */
@Composable
private fun SettingsPage(g: Generate) {
    FieldLabel("Server")
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.weight(1f)) {
            Field(g.server, "http://host:8188", lines = 1, keyboard = KeyboardType.Uri) { g.server = it }
        }
        Spacer(Modifier.width(8.dp))
        val connecting = g.connection == Connection.Connecting
        SmallButton(if (connecting) "…" else "Connect", enabled = !connecting && g.server.isNotBlank()) { g.connect() }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        val (dot, short) = connectionLine(g)
        Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        val words = when (val c = g.connection) {
            is Connection.Connected -> when {
                c.listed == 0 -> "Connected to $short: no workflows in workflows/procraft/"
                c.listed == c.usable -> "Connected to $short: ${c.listed} workflows"
                else -> "Connected to $short: ${c.usable} of ${c.listed} workflows usable"
            }
            is Connection.Failed -> c.why
            else -> short
        }
        Text(words, color = Label, fontSize = 13.sp, modifier = Modifier.padding(start = 6.dp))
    }
    Spacer(Modifier.height(6.dp))
    FieldLabel("Model (the server's workflows/procraft/)")
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Pc.ActionsGroup).padding(6.dp).heightIn(max = 220.dp).verticalScroll(rememberScrollState())) {
        if (g.workflows.isEmpty()) {
            val words = if (g.workflow.isNotEmpty()) "${g.workflow.removeSuffix(".json")} (Connect to list the others)" else "Connect to list the workflows"
            Text(words, color = Label, fontSize = 14.sp, modifier = Modifier.padding(10.dp))
        }
        for (w in g.workflows) {
            val name = w.name.removeSuffix(".json")
            val problem = w.problem
            if (problem != null) {
                Text("$name: $problem", color = Label, fontSize = 13.sp, modifier = Modifier.padding(10.dp))
            } else {
                val chosen = g.workflow == w.name
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(if (chosen) Pc.Accent else Color.Transparent)
                        .clickable { g.workflow = w.name }.padding(horizontal = 10.dp, vertical = 9.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(name, color = if (chosen) Color.White else Pc.Text, fontSize = 15.sp, maxLines = 1, modifier = Modifier.weight(1f))
                    Text(if (w.takesImage) "sketch → image" else "text → image", color = if (chosen) Color.White else Label, fontSize = 13.sp)
                }
            }
        }
    }
    if (g.workflow.isNotEmpty()) {
        Spacer(Modifier.height(6.dp))
        FieldLabel("Negative prompt for this model")
        Field(g.negatives[g.workflow] ?: "", "Empty: the workflow's own", lines = 3) { g.negatives[g.workflow] = it }
    }
}

@Composable
private fun connectionLine(g: Generate): Pair<Color, String> = when (g.connection) {
    Connection.Unknown -> Idle to "Not connected"
    Connection.Connecting -> ConnectingOrange to "Connecting…"
    is Connection.Connected -> DoneGreen to g.host
    is Connection.Failed -> FailedRed to "Offline"
}

@Composable
private fun FieldLabel(words: String) {
    Text(words, color = Label, fontSize = 14.sp, modifier = Modifier.padding(start = 2.dp, top = 2.dp))
}

/** A text field drawn as the panels' own: dark, rounded, the cursor blue. */
@Composable
private fun Field(value: String, hint: String, lines: Int, keyboard: KeyboardType = KeyboardType.Text, onChange: (String) -> Unit) {
    val style = TextStyle(color = Pc.Text, fontSize = 16.sp)
    BasicTextField(
        value,
        onChange,
        singleLine = lines == 1,
        minLines = lines,
        maxLines = if (lines == 1) 1 else lines + 4,
        textStyle = style,
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
        cursorBrush = SolidColor(Pc.Accent),
        modifier = Modifier.fillMaxWidth(),
        decorationBox = { inner ->
            Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(FieldFill).padding(horizontal = 12.dp, vertical = 10.dp)) {
                if (value.isEmpty()) Text(hint, style = style.copy(color = Label))
                inner()
            }
        },
    )
}

/** The strength, as the Actions panel's sliders look: its name and value over a thin track with a pill knob. */
@Composable
private fun StrengthSlider(value: Float, onChange: (Float) -> Unit) {
    val change by rememberUpdatedState(onChange)
    Box(
        Modifier.fillMaxWidth().height(60.dp).clip(RoundedCornerShape(12.dp)).background(Pc.ActionsGroup)
            .pointerInput(Unit) {
                val inset = 16.dp.toPx()
                fun at(x: Float) = (((x - inset) / (size.width - 2f * inset)).coerceIn(0f, 1f) * 100f).roundToInt() / 100f
                detectTapGestures { change(at(it.x)) }
            }
            .pointerInput(Unit) {
                val inset = 16.dp.toPx()
                fun at(x: Float) = (((x - inset) / (size.width - 2f * inset)).coerceIn(0f, 1f) * 100f).roundToInt() / 100f
                detectHorizontalDragGestures(onDragStart = { change(at(it.x)) }) { c, _ ->
                    c.consume()
                    change(at(c.position.x))
                }
            }
            .drawBehind {
                val inset = 16.dp.toPx()
                val y = size.height - 16.dp.toPx()
                val h = 3.dp.toPx()
                val x = inset + value.coerceIn(0f, 1f) * (size.width - 2f * inset)
                drawRect(Pc.SlideTrack, Offset(inset, y - h / 2f), Size(size.width - 2f * inset, h))
                drawRect(Pc.Accent, Offset(inset, y - h / 2f), Size(x - inset, h))
                val knob = Size(16.dp.toPx(), 10.dp.toPx())
                drawRoundRect(Color.White, Offset(x - knob.width / 2f, y - knob.height / 2f), knob, CornerRadius(knob.height / 2f))
            },
    ) {
        Text("Strength", color = Pc.SlideLabel, fontSize = 17.sp, modifier = Modifier.padding(start = 16.dp, top = 9.dp))
        Text("${(value * 100).roundToInt()}%", color = Pc.SlideValue, fontSize = 17.sp, modifier = Modifier.align(Alignment.TopEnd).padding(end = 16.dp, top = 9.dp))
    }
}

/**
 * The seed: random, a new one each time, the field showing the one last used; fixed, the one typed,
 * to keep a picture's composition while the prompt or strength changes.
 */
@Composable
private fun SeedRow(g: Generate) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Pc.ActionsGroup).padding(start = 16.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Seed", color = Pc.SlideLabel, fontSize = 17.sp)
        Spacer(Modifier.width(12.dp))
        val style = TextStyle(color = if (g.randomSeed) Label else Pc.Text, fontSize = 16.sp)
        BasicTextField(
            g.seed,
            { typed -> g.seed = typed.filter(Char::isDigit).take(19) },
            enabled = !g.randomSeed,
            singleLine = true,
            textStyle = style,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            cursorBrush = SolidColor(Pc.Accent),
            modifier = Modifier.weight(1f),
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(FieldFill).padding(horizontal = 10.dp, vertical = 8.dp)) { inner() }
            },
        )
        Spacer(Modifier.width(8.dp))
        // a tap switches between a new seed each time and the one shown
        Box(
            Modifier.height(36.dp).width(92.dp).clip(RoundedCornerShape(8.dp)).background(if (g.randomSeed) Pc.Accent else FieldFill)
                .clickable { g.randomSeed = !g.randomSeed },
            contentAlignment = Alignment.Center,
        ) {
            Text(if (g.randomSeed) "Random" else "Fixed", color = if (g.randomSeed) Color.White else Pc.Accent, fontSize = 15.sp, fontWeight = FontWeight.Medium)
        }
    }
}

@Composable
private fun WideButton(words: String, enabled: Boolean, onTap: () -> Unit) {
    Box(
        Modifier.fillMaxWidth().height(46.dp).clip(RoundedCornerShape(12.dp)).background(if (enabled) Pc.Accent else Pc.ActionsGroup)
            .clickable(enabled = enabled, onClick = onTap),
        contentAlignment = Alignment.Center,
    ) {
        Text(words, color = if (enabled) Color.White else Label, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun SmallButton(words: String, enabled: Boolean, onTap: () -> Unit) {
    Box(
        Modifier.height(42.dp).width(96.dp).clip(RoundedCornerShape(10.dp)).background(Pc.ActionsGroup).clickable(enabled = enabled, onClick = onTap),
        contentAlignment = Alignment.Center,
    ) {
        Text(words, color = if (enabled) Pc.Accent else Label, fontSize = 16.sp, fontWeight = FontWeight.Medium)
    }
}

/** How far the job has come: a thin line filling, or a short stretch sliding along while it can't be told. */
@Composable
private fun ProgressLine(progress: Float?) {
    val slide = if (progress == null) {
        rememberInfiniteTransition(label = "slide").animateFloat(0f, 1f, infiniteRepeatable(tween(1200, easing = LinearEasing)), label = "slide").value
    } else {
        0f
    }
    Box(
        Modifier.fillMaxWidth().height(6.dp).clip(CircleShape).background(Pc.SlideTrack).drawBehind {
            if (progress != null) {
                drawRect(Pc.Accent, Offset.Zero, Size(size.width * progress.coerceIn(0f, 1f), size.height))
            } else {
                val w = size.width * 0.3f
                drawRect(Pc.Accent, Offset(-w + slide * (size.width + w), 0f), Size(w, size.height))
            }
        },
    )
}
