# Generate (ComfyUI) — handoff

State on 2026-11-13. The **Generate** feature works end to end on the desktop app and on the Android tablet.
It lives on the **`comfy`** branch of the user's fork, `git@github.com:peko/procraft.git` (remote `origin`;
the original project is `upstream`, https://github.com/artcraft-android/procraft), rebased on the fork's `main`.

## What it is

A ComfyUI binding for ProCraft, a much simpler take on
[krita-ai-diffusion](../../krita-ai-diffusion) (the user's own fork, with MOSS support, is at `~/lab/krita-ai-diffusion`).
The user's flow:

1. Set the ComfyUI server address.
2. Choose one of the special workflows kept on the server.
3. Draw a sketch, type a prompt, choose a strength.
4. Press the button, and the result lands on the **AI layer** at the top (one undo step).
5. Keep it (✓) or throw it away (✕). Until then, each new render replaces the picture on the same layer,
   as krita-ai-diffusion's preview does.

It's native, not a plugin. ProCraft plugins (`docs/plugins.md`) are sandboxed wasm: they have no network or
clock, actions are capped at 20 s, and panels have no text input. The user agreed to the native approach. A
`comfyui` plugin permission could expose it to plugins later.

## Decisions the user made

- Keep it simple: no model, LoRA or workflow building in the app, only fixed workflows prepared on the server.
- Workflows are in **API format** ("Export (API)") under the server's `user/default/workflows/procraft/*.json`.
  Editor-format files (with subgraphs) are refused with a reason.
- Nodes are bound by title (`_meta.title`, one or several roles separated by spaces):
  `procraft:image` (LoadImage), `procraft:prompt`, `procraft:negative`, `procraft:strength`, `procraft:seed`
  (`seed`/`noise_seed`), `procraft:size` (width/height: the canvas proportions at about 1 MP, multiples of 16),
  `procraft:empty` (the empty latent), `procraft:encode` (the canvas's VAEEncode), `procraft:steps` (optional:
  the scheduler whose steps a `SplitSigmas` splits; otherwise found from its `sigmas` link).
- **Strength works as in krita-ai-diffusion** (`model/model.py:_prepare_workflow`, `backend/workflow.py`):
  - **100 % = Generate:** the sampler starts from `procraft:empty`. The canvas isn't uploaded, and the image
    chain is cut out of the graph.
  - **Below 100 % = Refine:** it starts from `procraft:encode`, and the empty latent is cut out.
  - The button and model list say Generate or Refine to match.
  - `procraft:strength` on a `SplitSigmas` (krita's graph) or a `KSamplerAdvanced` uses krita's
    `apply_strength`: `start = round(steps × (1 − s))`. When fewer than 4 steps would be left
    (`MIN_STEPS`), the schedule becomes `floor(4 / s)` steps, starting 4 from the end. So the model runs the
    last steps of its own schedule, which suits turbo/DMD models. On anything else it sets `denoise` (else
    `image_strength`, else `strength`), so older KSampler workflows still work.
  - One workflow per model covers both Generate and Refine. The old `*-sketch` / `*-t2i` pairs were removed
    from the repo and the server.
  - Without titles, the first LoadImage, the first text encoder, and every sampler's seed are used.
  - A prompt node whose text contains `{prompt}` keeps its text, with the prompt put in place of the
    placeholder (quality tags).
  - An empty negative keeps the workflow's own.
- **The AI layer** (asked for after the MVP, like krita-ai-diffusion):
  - A render goes onto the top paint layer named exactly `AI` (`comfy::PREVIEW`). The first render makes that
    layer at the very top, outside any group, and the user's layer stays current. Each later render replaces
    its pixels, one undo step each.
  - The AI layer is left out of the sketch sent to ComfyUI (`finished_picture_without`, which hides it only
    while the canvas is read).
  - The AI button sits at the very right of the bar, after the colour. Under it, ✕ then ✓ discs hang one over
    the other while a picture waits and no job runs, where a window's close is (the badge shows during a job). The panel shows Discard and Keep under "Generate again".
  - **Keep** (✓) copies the AI layer to a new layer just under it, named "AI: <prompt>". The AI layer stays on
    top as the target for the next render, and the user's layer stays current. **Discard** (✕) deletes the AI
    layer. There is only ever one AI layer.
  - ✕/✓ show while a new render is waiting (`pending` in the UI). They go away after keep or discard, or when
    the AI layer disappears (undo, or deleted by hand). The AI layer itself is found by name.
- MVP is text → image and image → image (refine). There is no inpainting or Live mode yet.
- Desktop first, then Android. Both are done.
- Button: a **tap** generates with the last settings. If no workflow is chosen yet, or a job is running, the
  tap opens the panel instead. A **hold** (0.4 s) opens the panel.
  - The button is animated: a spinning arc while queued, a filling ring while running, a pulsing sparkle, and
    a green or red ring for 2 s when the job ends.
  - With the panel closed, a badge under the button says Queued, 42%, Done or Failed.
- Panel: two tabs.
  - **Generate:** the model row (connection dot and host; tapping it opens Settings), prompt, strength (only
    unless the model can't refine), seed (Random/Fixed), Generate or Refine (by strength) / Cancel with %, and a status line.
  - **Settings** (gear): server + Connect + status, the model list, **Send: Canvas / Current layer**, and a
    negative prompt per model.
- **Send** (`ComfyRequest::source`, `ComfySource`):
  - **Canvas** sends the finished picture without the AI layer.
  - **Current layer** sends that layer's own pixels (`State::layer_pixels`, un-premultiplied), whether it's
    shown or not. A group is refused with a reason.
  - Either way the picture is flattened onto white and sent as PNG (≤ 2048 px). The user wants the PNG kept
    as is for now.

## Code map

| Where | What |
|---|---|
| `crates/core/src/comfy/client.rs` | HTTP (`ureq` 3.5, rustls+json) and websocket (`tungstenite` 0.30) client: list workflows (`GET /api/userdata?dir=workflows/procraft&recurse=true`), fetch one, multipart upload `/upload/image`, `POST /prompt`, `/history/{id}`, `/view`, cancel (`POST /queue {delete}` + `POST /interrupt {prompt_id}`), ComfyUI error messages made readable |
| `crates/core/src/comfy/workflow.rs` | Pure binding logic (`describe`, `apply`, `megapixel_size`), unit-tested |
| `crates/core/src/comfy/mod.rs` | The uniffi API: `comfy_workflows(server)`, `CanvasSession::comfy_generate(ComfyRequest) -> Arc<ComfyJob>` (reads the canvas on the calling thread via `finished_picture`, which was made `pub(crate)` in `project.rs`, then runs on a thread), `ComfyJob::{status, cancel, seed}`, `CanvasSession::{comfy_put_result, comfy_preview, comfy_accept(prompt), comfy_discard}` (the AI layer, one undo step through `plugin_step`), `ComfyJob::prompt`. Progress is weighted as krita does: 20 % nodes, 80 % sampler steps, never going back. Tests include ignored live tests `comfy_live` and `comfy_list_live` |
| `crates/core/src/lib.rs` | `mod comfy;` and `pub use comfy::{…}` |
| `desktop/src/generate.rs` | The desktop panel, `SPARKLE`/`GEAR` icons, the bar ring (`draw_status`, `pulse`), the badge, and saved settings in `~/.local/share/procraft/generate.json` |
| `desktop/src/{chrome,ui,look,main}.rs` | `BarButton::Generate`, `Popover::Generate`, the hold via `tool_hold`, bar spacing for 5 action buttons |
| `android/.../canvas/Generate.kt` | Android state, saved in SharedPreferences "generate"; follows the job on `viewModelScope` |
| `android/.../ui/GenerateUi.kt` | `GenerateButton`, `GenerateBadge`, `GeneratePanel` (Compose) |
| `android/.../canvas/CanvasViewModel.kt` | `Panel.GENERATE`, `val generate`, `putGenerated`, `generatePressed` |
| `android/.../ui/{Chrome,AppRoot,Theme}.kt` | Button at the bar's right end (`toolCentreFromRight(-1)`), panel placement, `BarFit.needs` counting 4 actions + 6 right buttons |
| `android/app/src/main/AndroidManifest.xml` | `INTERNET` + `usesCleartextTraffic="true"` |
| `tools/comfy-workflows/*.json` | The four workflows installed on the server (copies) |

## Environment (this machine)

- Linux (Manjaro, X11), AMD Radeon 680M.
- Desktop build: `cargo build --release -p procraft-desktop`, run `./target/release/procraft`.
- Rust: user-local rustup is now installed (`~/.cargo/bin`, stable + `aarch64-linux-android` + clippy), and
  cargo prefers it over Arch's `rust` 1.99.
- `cargo-ndk` 4.1.2 is in `~/.cargo/bin`.
- Android SDK at `~/Android/Sdk` (cmdline-tools, `platforms;android-37.0`, `build-tools;37.0.0`,
  platform-tools). The NDK 29.0.14206865 is linked from `/opt/android-sdk/ndk`.
  `android/local.properties` has `sdk.dir` (gitignored).
- Android build:
  `cd android && JAVA_HOME=/usr/lib/jvm/default ANDROID_HOME=$HOME/Android/Sdk ./gradlew assembleDebug -Pprocraft.abis=arm64-v8a`,
  then `adb install -r app/build/outputs/apk/debug/app-debug.apk`. The package is
  `io.github.artcraftandroid.procraft.debug`.
- Tablet: MediaTek "ArtPadPro_ROW", Android 16 (API 36), arm64, 1600×2176, on Wi-Fi at 192.168.1.108 with
  USB debugging authorized. It locks when asleep, so ask the user to unlock it before screenshots
  (`adb exec-out screencap -p`).
- Clippy is clean for `procraft-core` and `procraft-desktop`. 12 unit tests pass:
  `cargo test -p procraft-core comfy`.
  Live tests: `COMFY_URL=http://10.0.0.214:8188 COMFY_WORKFLOW=$PWD/tools/comfy-workflows/anima.json COMFY_SKETCH=<png> COMFY_STRENGTH=0.85 cargo test -p procraft-core comfy_live -- --ignored --nocapture`.

## The ComfyUI server

- `10.0.0.214:8188` (wired LAN). ComfyUI 0.43.0, RTX 5060 Ti 16 GB, at `peko@10.0.0.214:~/lab/Diffusion/ComfyUI`.
  SSH works. Copying files there was approved; other remote writes need the user's OK.
- Workflows for ProCraft: `~/lab/Diffusion/ComfyUI/user/default/workflows/procraft/`, holding **`moss` and
  `anima`**.
- The two workflows are **built by krita-ai-diffusion's own code** (`load_checkpoint_with_lora`,
  `empty_latent_image`, `sampler_custom_advanced`: BasicGuider, BasicScheduler → SplitSigmas, RandomNoise,
  KSamplerSelect, SamplerCustomAdvanced). Its websocket nodes are swapped for LoadImage / SaveImage, and the
  graphs are then titled for ProCraft. To rebuild them: `python3 tools/comfy-workflows/make.py` (it runs
  `krita_graph.py` with krita-ai-diffusion's venv), then copy them to the server.
- **MOSS:**
  - Model `moss/moss_image_sft_int8_convrot` + LoRA `moss/turbo_sft_8step_dmd_v0_1`, CLIP
    `moss/qwen3.5_9b_int8_convrot` (type `moss_image`), VAE `flux2-vae`, 8 steps, CFG 1, euler/simple.
  - Includes `SkipLayerGuidanceDiTSimple` single_layers "22" (from the user's krita fork; only acts at CFG > 1).
  - With krita's strength on a line sketch: 30–80 % is the sketch, cleaned or tinted; 90 % (7 of 8 steps) a
    picture following its layout on the sketch's white; 100 % a new picture. Turbo MOSS jumps between 80 and
    90 %.
- **Anima:**
  - Model `anima/anima_turboV11` (turbo built in), CLIP `qwen306BBaseAnima_1` (type `omnigen2` as krita has it;
    `stable_diffusion` gives the same picture), VAE
    `qwen_image_vae`, 8 steps, CFG 1, euler/simple.
  - Positive `masterpiece, best quality, score_7, highres, safe, {prompt}`; negative `worst quality, low
    quality, score_1, score_2, score_3, blurry, jpeg artifacts, sepia`.
  - Non-turbo Anima models need about 30 steps at CFG 4.
- Timing: about 4–5 s per job with the model loaded, about 27–70 s when it has to load first.
- **Tablet access:** the tablet is on Wi-Fi 192.168.1.0/24 and can't reach 10.0.0.x directly.
  `~/ssh-local.sh` (rewritten this session) relays ports 8188/8000/5001 over SSH on `0.0.0.0` of this PC.
  - The tablet uses **`192.168.1.227:8188`**, this PC's Wi-Fi address from DHCP, which can change.
  - `~/ssh-local.sh relay` runs the relay only and reconnects by itself.
  - Verified working: the user generated from the tablet through it.

## Not done yet / next steps

1. **Check the AI layer's ✕/✓ by eye** on desktop and on the tablet. Both are built; the logic is unit-tested
   (`pictures_wait_on_one_top_layer_until_kept_or_thrown_away`). The screen was locked and the tablet was
   unplugged when it was written.
2. **Check the Seed row on the tablet** (installed, unseen because the tablet was locked) and on desktop (built,
   not run).
3. Further work goes on `comfy`, with semantic commit messages in the project's style
   (`feat(scope): what it does, in words`).
4. Ideas not yet asked for:
   - inpainting inside the selection;
   - Live mode;
   - right-click on desktop to open the panel (proposed, not answered);
   - a MOSS edit workflow (`TextEncodeMOSSImageEdit` with `edit_format "Moss-Image-Edit"`, `images.image_0`,
     the edit adapter LoRA, denoise 1);
   - more model workflows (copy a JSON and change `unet_name`);
   - a `comfyui` plugin permission.
5. `desktop/src/icons.rs` is generated from Android's `ui/Icons.kt`, so the Generate icons live in
   `generate.rs` and `GenerateUi.kt` instead.

## Cautions learned this session

- Never drive the desktop with xdotool blindly. A click once landed in another window. Use a guard that checks
  `xdotool getmouselocation` is over ProCraft's window before each click.
- Read files before overwriting them. `~/.local/share/procraft/generate.json` was once overwritten unread.
- `.private/` folders are off-limits (global rule).
- On desktop, a tap on the Generate button with a workflow chosen starts a real job. Hold to open the panel.
