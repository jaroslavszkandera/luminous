import logging as log
from pathlib import Path

import open_clip
import pyarrow as pa
import torch
from PIL import Image
from rich.progress import (
    BarColumn,
    MofNCompleteColumn,
    Progress,
    SpinnerColumn,
    TextColumn,
    TimeElapsedColumn,
    TimeRemainingColumn,
)

import lancedb

log.basicConfig(format="[CLIP]:%(asctime)s:%(levelname)s:%(message)s", level=log.INFO)
log.getLogger("PIL").setLevel(log.WARNING)
log.getLogger("urllib3").setLevel(log.WARNING)

IMAGE_FOLDER = Path("/home/jarek/Pictures/wallpapers")
VALID_EXT = {".jpg", ".jpeg", ".png", ".webp"}
EMBED_DIM = 512
MODEL = "MobileCLIP-B"
PRETRAINED = "datacompdr_lt"
DB_PATH = "./lancedb"
THRESHOLD = 1.50
DIM_THRESHOLD = 1.60


device = "cuda" if torch.cuda.is_available() else "cpu"
log.info(f"Using {device=}")

model, _, preprocess = open_clip.create_model_and_transforms(
    MODEL, pretrained=PRETRAINED, device=device
)
model.eval()
tokenizer = open_clip.get_tokenizer(MODEL)

db = lancedb.connect(DB_PATH)

SCHEMA = pa.schema(
    [
        pa.field("path", pa.utf8()),
        pa.field("embedding", pa.list_(pa.float32(), EMBED_DIM)),
    ]
)


def build_index(folder: Path) -> None:
    paths = [p for p in folder.glob("*") if p.suffix.lower() in VALID_EXT]

    if "images" in db.table_names():
        tbl = db.open_table("images")
        indexed = {row["path"] for row in tbl.to_arrow().to_pylist()}
        paths = [p for p in paths if str(p) not in indexed]
    else:
        tbl = db.create_table("images", schema=SCHEMA)

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        MofNCompleteColumn(),
        TimeElapsedColumn(),
        TimeRemainingColumn(),
        TextColumn("[dim]{task.fields[img]}"),
    ) as progress:
        task = progress.add_task("Indexing", total=len(paths), img="")
        for p in paths:
            progress.update(task, img=p.name)
            try:
                img = preprocess(Image.open(p)).unsqueeze(0).to(device)
                with torch.no_grad():
                    vec = model.encode_image(img).float()
                    vec /= vec.norm(dim=-1, keepdim=True)
                tbl.add(
                    [
                        {
                            "path": str(p),
                            "embedding": vec.squeeze().cpu().numpy().tolist(),
                        }
                    ]
                )
            except Exception as e:
                log.warning(f"Skipped {p.name}: {e}")
            progress.advance(task)

    log.info(f"Done. Table has {tbl.count_rows()} images.")


def repl() -> None:
    tbl = db.open_table("images")
    while query := input("\nSearch: ").strip():
        if query in ("exit", "quit"):
            break
        with torch.no_grad():
            tokens = tokenizer([f"a photo of {query}"]).to(device)
            vec = model.encode_text(tokens).float()
            vec /= vec.norm(dim=-1, keepdim=True)
        results = tbl.search(vec.squeeze().cpu().numpy()).limit(20).to_list()
        results.sort(key=lambda r: r["_distance"])

        good = [r for r in results if r["_distance"] <= THRESHOLD]
        dim = [r for r in results if THRESHOLD < r["_distance"] <= DIM_THRESHOLD]

        print(f"\nResults for '{query}':")
        if len(good) == 0:
            print("No results")
        for r in good:
            print(f"  [{r['_distance']:.4f}] {Path(r['path']).name}")
        if dim:
            print("\n  \033[90m── below threshold ──")
            for r in dim:
                print(f"  [{r['_distance']:.4f}] {Path(r['path']).name}")
            print("\033[0m", end="")


def main() -> None:
    try:
        build_index(IMAGE_FOLDER)
        repl()
    except (KeyboardInterrupt, EOFError):
        print("Interrupt, exiting...")


if __name__ == "__main__":
    main()
