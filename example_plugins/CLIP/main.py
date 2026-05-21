import json
import logging as log
import queue
import socket
import struct
import threading
from pathlib import Path

import lancedb
import open_clip
import pyarrow as pa
import torch
from PIL import Image
from platformdirs import user_cache_dir

HOST = "127.0.0.1"
PORT = 50022

VALID_EXT = {".jpg", ".jpeg", ".png", ".webp"}
EMBED_DIM = 512
MODEL = "MobileCLIP-B"
PRETRAINED = "datacompdr_lt"


def get_db_path() -> str:
    cache_dir = Path(user_cache_dir("luminous", appauthor=False)) / "CLIP" / "lancedb"
    cache_dir.mkdir(parents=True, exist_ok=True)
    abs_path = str(cache_dir.absolute())
    log.info(f"Database directory resolved to: {abs_path}")
    return abs_path


THRESHOLD = 1.70
DIM_THRESHOLD = 1.80


def recv_msg(conn: socket.socket) -> dict | None:
    try:
        header = conn.recv(4)
        if not header:
            return None
        msg_len = struct.unpack(">I", header)[0]
        chunks = []
        received = 0
        while received < msg_len:
            chunk = conn.recv(min(msg_len - received, 4096))
            if not chunk:
                raise RuntimeError("Connection broken")
            chunks.append(chunk)
            received += len(chunk)
        return json.loads(b"".join(chunks))
    except Exception:
        return None


def send_resp(conn: socket.socket, data: dict) -> None:
    payload = json.dumps(data).encode()
    conn.sendall(struct.pack(">I", len(payload)) + payload)


class Worker:
    def __init__(self, model, tokenizer, preprocess, device: str) -> None:
        self.model = model
        self.tokenizer = tokenizer
        self.preprocess = preprocess
        self.device = device
        self.db_path = get_db_path()
        self.db = lancedb.connect(self.db_path)
        self._db_table = None
        self._indexing = False
        self._queue = queue.Queue()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while True:
            item = self._queue.get()
            if item is None:
                log.debug("item is not exiting proc loop")
                break
            paths = item
            try:
                self._handle_index(paths)
            except Exception as e:
                log.error(f"Indexing error: {e}")
                self._indexing = False

    def _handle_index(self, paths: list[str]) -> None:
        self._indexing = True
        valid_paths = [Path(p) for p in paths if Path(p).suffix.lower() in VALID_EXT]

        if "images" in self.db.table_names():
            table = self.db.open_table("images")
            indexed = {row["path"] for row in table.to_arrow().to_pylist()}
            valid_paths = [p for p in valid_paths if str(p) not in indexed]
        else:
            schema = pa.schema(
                [
                    pa.field("path", pa.utf8()),
                    pa.field("embedding", pa.list_(pa.float32(), EMBED_DIM)),
                ]
            )
            table = self.db.create_table("images", schema=schema)

        total = len(valid_paths)
        if total == 0:
            self._indexing = False
            return

        log.info(f"Indexing {total} images...")
        processed = 0
        batch = []

        for p in valid_paths:
            try:
                img = self.preprocess(Image.open(p)).unsqueeze(0).to(self.device)
                with torch.no_grad():
                    vec = self.model.encode_image(img).float()
                    vec /= vec.norm(dim=-1, keepdim=True)

                batch.append(
                    {
                        "path": str(p),
                        "embedding": vec.squeeze().cpu().numpy().tolist(),
                    }
                )
                processed += 1

                if len(batch) >= 5:
                    table.add(batch)
                    log.info(
                        f"Flushed {len(batch)} entries to db. Progress: {processed}/{total}"
                    )
                    batch = []
            except Exception as e:
                log.warning(f"Failed {p.name}: {e}")

        if batch:
            table.add(batch)
            log.info(f"Flushed final {len(batch)} entries to db.")

        self._db_table = table
        self._indexing = False
        log.info(f"Indexing finished. Total rows: {table.count_rows()}")

    def enqueue_index(self, paths: list[str]) -> None:
        self._queue.put(paths)

    def is_ready(self) -> bool:
        return self._db_table is not None

    def ensure_table(self) -> None:
        if self._db_table is None and "images" in self.db.table_names():
            self._db_table = self.db.open_table("images")

    def search(self, query: str, limit: int = 20) -> list[dict]:
        with torch.no_grad():
            tokens = self.tokenizer([f"a photo of {query}"]).to(self.device)
            vec = self.model.encode_text(tokens).float()
            vec /= vec.norm(dim=-1, keepdim=True)

        results = (
            self._db_table.search(vec.squeeze().cpu().numpy()).limit(limit).to_list()
        )
        return [r for r in results if r["_distance"] <= DIM_THRESHOLD]


def handle_connection(conn: socket.socket, addr: tuple, worker: Worker) -> None:
    log.info(f"Host connected from {addr}")
    conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    with conn:
        while True:
            cmd = recv_msg(conn)
            if cmd is None:
                log.info("Host disconnected.")
                break

            try:
                action = cmd.get("action")
                log.debug(f"{action=}")

                if action == "ping":
                    send_resp(conn, {"status": "ok"})

                elif action == "index":
                    worker.enqueue_index(cmd.get("paths", []))
                    send_resp(conn, {"status": "ok"})

                elif action == "search":
                    worker.ensure_table()
                    paths = []
                    if worker.is_ready():
                        results = worker.search(cmd["query"], cmd.get("limit", 20))
                        paths = [
                            r["path"] for r in results if r["_distance"] <= THRESHOLD
                        ]

                    send_resp(conn, {"SearchResult": {"paths": paths}})

                    if "paths" in cmd:
                        worker.enqueue_index(cmd["paths"])

                elif action == "shutdown":
                    send_resp(conn, {"status": "ok"})
                    return

                else:
                    log.error(f"Unknown action: {action}")
                    send_resp(
                        conn,
                        {"status": "error", "message": f"Unknown action: {action}"},
                    )

            except Exception as e:
                log.error(f"Processing error: {e}")
                send_resp(conn, {"status": "error", "message": str(e)})


def main() -> None:
    log.basicConfig(
        format="[CLIP]:%(asctime)s:%(levelname)s:%(message)s", level=log.DEBUG
    )
    log.getLogger("PIL").setLevel(log.WARN)

    device = "cuda" if torch.cuda.is_available() else "cpu"
    log.info(f"Using {device=}")

    log.info("Loading CLIP model...")
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL, pretrained=PRETRAINED, device=device
    )
    model.eval()
    tokenizer = open_clip.get_tokenizer(MODEL)

    worker = Worker(model, tokenizer, preprocess, device)

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind((HOST, PORT))
            srv.listen(1)
            log.info(f"CLIP daemon listening on {HOST}:{PORT}")
            conn, addr = srv.accept()
            handle_connection(conn, addr, worker)
    except (OSError, KeyboardInterrupt) as e:
        log.error(f"Server error: {e}")
    finally:
        log.info("CLIP daemon exiting...")


if __name__ == "__main__":
    main()
