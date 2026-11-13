# MOSS and Anima graphs from krita-ai-diffusion's own building blocks, chained as its refine() and
# generate() chain them, with plain LoadImage / SaveImage in place of its websocket nodes. Both
# latent sources are in the graph; ProCraft wires the sampler to one (empty at 100 %).
import sys, json
sys.path.insert(0, sys.argv[2] if len(sys.argv) > 2 else __import__("os").path.expanduser("~/lab/krita-ai-diffusion"))
from ai_diffusion.backend.comfy_workflow import ComfyWorkflow
from ai_diffusion.backend.client import ClientModels, CheckpointInfo
from ai_diffusion.backend.resources import Arch, ResourceKind, resource_id
from ai_diffusion.backend.api import CheckpointInput, LoraInput
from ai_diffusion.backend import workflow
from ai_diffusion.files import FileFormat
from ai_diffusion.image import Extent

SETUPS = {
    "moss": dict(arch=Arch.moss, unet="moss/moss_image_sft_int8_convrot.safetensors",
                 te=("qwen_35_9b", "moss/qwen3.5_9b_int8_convrot.safetensors"), vae="flux2-vae.safetensors",
                 loras=[LoraInput("moss/turbo_sft_8step_dmd_v0_1.safetensors", 1.0)],
                 prompt="{prompt}", negative=""),
    "anima": dict(arch=Arch.anima, unet="anima/anima_turboV11.safetensors",
                  te=("qwen_3_06b", "qwen306BBaseAnima_1.safetensors"), vae="qwen_image_vae.safetensors", loras=[],
                  prompt="masterpiece, best quality, score_7, highres, safe, {prompt}",
                  negative="worst quality, low quality, score_1, score_2, score_3, blurry, jpeg artifacts, sepia"),
}

def build(name):
    s = SETUPS[name]; arch = s["arch"]
    m = ClientModels()
    m.checkpoints = {"M": CheckpointInfo(s["unet"], arch, FileFormat.diffusion)}
    m.resources[resource_id(ResourceKind.text_encoder, Arch.all, s["te"][0])] = s["te"][1]
    m.resources[resource_id(ResourceKind.vae, arch, "default")] = s["vae"]
    w = ComfyWorkflow()
    model, clip, vae = workflow.load_checkpoint_with_lora(w, CheckpointInput("M", arch, loras=s["loras"]), m)
    pos = w.clip_text_encode(clip.model, s["prompt"])
    neg = w.clip_text_encode(clip.model, s["negative"])
    cond = workflow.ConditioningOutput(pos, neg)
    # refine's input: the canvas at about a megapixel, encoded
    image = w.add("LoadImage", 2, image="procraft_canvas.png")[0]
    image = w.add("ImageScaleToTotalPixels", 1, image=image, upscale_method="lanczos", megapixels=1.0, resolution_steps=16)
    encoded = w.vae_encode(vae, image)
    # generate's input
    empty = w.empty_latent_image(Extent(1024, 1024), arch)
    # start_at_step 1 makes it add its SplitSigmas (ProCraft sets the step, 0 at 100 %)
    out = w.sampler_custom_advanced(model, cond, encoded, arch, sampler="euler", scheduler="simple",
                                    steps=8, start_at_step=1, cfg=1.0, seed=0, extent=Extent(1024, 1024))
    decoded = w.vae_decode(vae, out)
    w.add("SaveImage", 0, images=decoded, filename_prefix=f"procraft/{name}")
    return w.root, dict(pos=pos.node, neg=neg.node, load=image and None, encoded=encoded.node, empty=empty.node)

if __name__ == "__main__":
    root, ids = build(sys.argv[1])
    print(json.dumps(root, indent=1))
