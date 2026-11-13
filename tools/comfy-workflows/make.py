"""Writes moss.json and anima.json: the graphs krita_graph.py builds with krita-ai-diffusion's code,
titled for ProCraft. Run from anywhere: python3 tools/comfy-workflows/make.py [path to krita-ai-diffusion]"""
import json, os, subprocess, sys
WF = os.path.dirname(os.path.abspath(__file__))
KRITA = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/lab/krita-ai-diffusion")
TITLES = {
    "UNETLoader": "Model", "CLIPLoader": "Text encoder", "VAELoader": "VAE", "LoraLoader": "Turbo LoRA (8 steps, CFG 1)",
    "SkipLayerGuidanceDiTSimple": "Skip layer 22 (MOSS recipe; acts only with CFG > 1)",
    "LoadImage": "procraft:image", "ImageScaleToTotalPixels": "Canvas at about 1 MP", "VAEEncode": "procraft:encode",
    "BasicGuider": "Guider (CFG 1)", "BasicScheduler": "procraft:steps", "SplitSigmas": "procraft:strength",
    "RandomNoise": "procraft:seed", "KSamplerSelect": "Sampler", "SamplerCustomAdvanced": "Sample",
    "VAEDecode": "Decode", "SaveImage": "Save",
}
for name in ("moss", "anima"):
    root = json.loads(subprocess.check_output([f"{KRITA}/venv/bin/python", f"{WF}/krita_graph.py", name, KRITA]))
    texts = [k for k, v in root.items() if v["class_type"] == "CLIPTextEncode"]
    for k, v in root.items():
        c = v["class_type"]
        title = TITLES.get(c, c)
        if c.startswith("Empty"): title = "procraft:size procraft:empty"
        if c == "CLIPTextEncode": title = "procraft:prompt" if k == texts[0] else "procraft:negative"
        v["_meta"] = {"title": title}
    json.dump(root, open(f"{WF}/{name}.json", "w"), indent=2, ensure_ascii=False)
    print(name, "written")
