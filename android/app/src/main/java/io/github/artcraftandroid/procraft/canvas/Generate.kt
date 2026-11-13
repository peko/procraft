package io.github.artcraftandroid.procraft.canvas

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.viewModelScope
import io.github.artcraftandroid.procraft.core.ComfyJob
import io.github.artcraftandroid.procraft.core.ComfyPhase
import io.github.artcraftandroid.procraft.core.ComfyRequest
import io.github.artcraftandroid.procraft.core.ComfySource
import io.github.artcraftandroid.procraft.core.ComfyWorkflow
import io.github.artcraftandroid.procraft.core.LayerType
import io.github.artcraftandroid.procraft.core.comfyWorkflows
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** How the Generate panel stands with the server. */
sealed interface Connection {
    /** Not asked yet this run. */
    data object Unknown : Connection
    data object Connecting : Connection
    /** It answered, listing these many workflows (and these many usable). */
    data class Connected(val listed: Int, val usable: Int) : Connection
    data class Failed(val why: String) : Connection
}

/** How the bar's button shows the job: idle, under way (its share done, if known), or how the last one ended, for a moment. */
sealed interface BarStatus {
    data object Idle : BarStatus
    data class Working(val progress: Float?) : BarStatus
    data object Done : BarStatus
    data object Failed : BarStatus
}

/**
 * Generate: the canvas and a prompt sent to a ComfyUI server through one of the workflows kept there
 * for ProCraft (its `workflows/procraft/`), the picture taken back onto the "AI" layer at the top,
 * where it waits to be kept ([accept]) or thrown away ([discard]), each new one over the last. The core does the
 * talking ([comfyWorkflows], `comfyGenerate`); this keeps the panel's fields (between runs, in the
 * "generate" preferences) and follows the job.
 */
class Generate(private val vm: CanvasViewModel, context: Context) {
    private val prefs = context.getSharedPreferences("generate", Context.MODE_PRIVATE)

    var server by mutableStateOf(prefs.getString("server", "") ?: "")
    var workflow by mutableStateOf(prefs.getString("workflow", "") ?: "")
    var prompt by mutableStateOf(prefs.getString("prompt", "") ?: "")
    var strength by mutableStateOf(prefs.getFloat("strength", 0.9f))
    /** The seed as typed (the last used while it is random), and whether each job takes a new one. */
    var seed by mutableStateOf(prefs.getString("seed", "0") ?: "0")
    var randomSeed by mutableStateOf(prefs.getBoolean("randomSeed", true))
    /** What is sent: the current layer alone (true) or the whole canvas. */
    var currentLayer by mutableStateOf(prefs.getBoolean("currentLayer", false))
    /** Each workflow's negative prompt, by its name (none: the workflow's own). */
    val negatives = mutableStateMapOf<String, String>().apply {
        val kept = runCatching { JSONObject(prefs.getString("negatives", "{}") ?: "{}") }.getOrNull()
        kept?.keys()?.forEach { k -> put(k, kept.optString(k)) }
    }

    var workflows by mutableStateOf<List<ComfyWorkflow>>(emptyList())
        private set
    var connection by mutableStateOf<Connection>(Connection.Unknown)
        private set
    /** How the last job went, or the one under way goes. */
    var status by mutableStateOf("")
        private set
    var progress by mutableStateOf<Float?>(null)
        private set
    private var job by mutableStateOf<ComfyJob?>(null)
    private var following: Job? = null
    /** How the last job ended (true: it made a layer), and when (uptime ms). */
    private var outcome by mutableStateOf<Pair<Boolean, Long>?>(null)

    val busy: Boolean get() = job != null

    /** A job now refines the canvas (below 100 %), else makes a picture new, as krita-ai-diffusion's Refine and Generate. */
    val refines: Boolean get() = strength < 1f && (chosen?.takesImage ?: true)

    /** A new picture was put on the AI layer and isn't kept or thrown away yet. */
    private var waiting by mutableStateOf(false)

    /** A new picture waits on the AI layer to be kept or thrown away (none once undo or a hand takes the layer away). */
    val pending: Boolean get() = waiting && vm.layers.any { it.kind == LayerType.PAINT && it.name == PREVIEW_LAYER }

    /** The prompt the picture waiting was made from, to name its layer when kept. */
    private var landedPrompt = ""

    /** Keeps the picture waiting: its layer stays, named for its prompt. */
    fun accept() {
        if (vm.acceptGenerated(landedPrompt.ifBlank { prompt.trim() })) {
            status = "Kept: copied to a layer of its own"
            waiting = false
        }
    }

    /** Throws the picture waiting away, with its layer. */
    fun discard() {
        if (vm.discardGenerated()) status = "Thrown away"
        waiting = false
    }

    /** The chosen workflow as the server listed it. */
    val chosen: ComfyWorkflow? get() = workflows.firstOrNull { it.name == workflow }

    /** Lists the server's workflows, off the interface's thread. */
    fun connect() {
        if (connection == Connection.Connecting) return
        connection = Connection.Connecting
        keep()
        val address = server.trim()
        vm.viewModelScope.launch {
            val listed = withContext(Dispatchers.IO) { runCatching { comfyWorkflows(address) } }
            listed.fold(
                onSuccess = { list ->
                    val usable = list.count { it.problem == null }
                    connection = Connection.Connected(list.size, usable)
                    // the one chosen stays chosen if it's still there, else the first usable
                    if (list.none { it.name == workflow && it.problem == null }) {
                        workflow = list.firstOrNull { it.problem == null }?.name ?: ""
                    }
                    workflows = list
                    keep()
                },
                onFailure = { e -> connection = Connection.Failed(reason(e)) },
            )
        }
    }

    /** Connects if the server wasn't asked yet this run (the panel opening). */
    fun connectOnce() {
        if (connection == Connection.Unknown && server.isNotBlank()) connect()
    }

    /** Sends the canvas and the prompt through the chosen workflow, and follows the job to its layer. */
    fun start() {
        val s = vm.session ?: return
        if (busy || workflow.isEmpty()) return
        val request = ComfyRequest(
            server = server.trim(),
            workflow = workflow,
            prompt = prompt.trim(),
            negative = negatives[workflow]?.trim() ?: "",
            strength = strength,
            seed = if (randomSeed) null else typedSeed(),
            source = if (currentLayer) ComfySource.CURRENT_LAYER else ComfySource.CANVAS,
        )
        val started = s.comfyGenerate(request)
        // the seed it uses shows at once, to keep if the picture is liked
        seed = started.seed().toString()
        job = started
        progress = null
        outcome = null
        status = "Sending…"
        keep()
        following = vm.viewModelScope.launch { follow(started) }
    }

    /** The seed as typed: its digits, or 0. */
    private fun typedSeed(): ULong = seed.filter(Char::isDigit).take(19).toULongOrNull() ?: 0uL

    fun cancel() {
        job?.cancel()
        status = "Calling it off…"
    }

    /** Asks [j] how it goes until it ends; its picture, once made, goes on a new layer. */
    private suspend fun follow(j: ComfyJob) {
        while (true) {
            val st = withContext(Dispatchers.Default) { j.status() }
            when (st.phase) {
                ComfyPhase.DONE -> {
                    val sent = j.prompt()
                    val put = vm.putGenerated(j)
                    if (put) {
                        landedPrompt = sent
                        waiting = true
                    }
                    ended(put, if (put) "${st.message}: on the AI layer. Keep it or throw it away" else "The picture couldn't go on a layer")
                    return
                }
                ComfyPhase.FAILED -> {
                    if ("refused" in st.message || "timed out" in st.message) connection = Connection.Failed("The server didn't answer")
                    ended(false, st.message)
                    return
                }
                ComfyPhase.CANCELLED -> {
                    ended(false, "Called off")
                    outcome = null
                    return
                }
                else -> {
                    progress = st.progress.takeIf { it >= 0f }
                    status = st.message
                }
            }
            delay(100)
        }
    }

    private fun ended(ok: Boolean, words: String) {
        job?.destroy()
        job = null
        progress = null
        outcome = ok to android.os.SystemClock.uptimeMillis()
        status = words
    }

    /** How the bar's button shows the job now (the outcome for two seconds after it ends). */
    fun barStatus(now: Long): BarStatus {
        if (busy) return BarStatus.Working(progress)
        val (ok, at) = outcome ?: return BarStatus.Idle
        return when {
            now - at > OUTCOME_SHOWN_MS -> BarStatus.Idle
            ok -> BarStatus.Done
            else -> BarStatus.Failed
        }
    }

    /** Writes what the panel keeps. */
    fun keep() {
        val kept = JSONObject()
        negatives.filterValues { it.isNotBlank() }.forEach { (k, v) -> kept.put(k, v) }
        prefs.edit()
            .putString("server", server.trim())
            .putString("workflow", workflow)
            .putString("prompt", prompt)
            .putFloat("strength", strength)
            .putString("seed", typedSeed().toString())
            .putBoolean("randomSeed", randomSeed)
            .putBoolean("currentLayer", currentLayer)
            .putString("negatives", kept.toString())
            .apply()
    }

    /** The server's host and port, without its scheme. */
    val host: String get() = server.trim().substringAfter("://").trimEnd('/')

    companion object {
        const val OUTCOME_SHOWN_MS = 2000L

        /** The name of the layer a picture waits on (the core's `comfy::PREVIEW`). */
        const val PREVIEW_LAYER = "AI"

        private fun reason(e: Throwable): String = when (e) {
            is io.github.artcraftandroid.procraft.core.ComfyException.Failed -> e.reason
            else -> e.message ?: e.toString()
        }
    }
}
