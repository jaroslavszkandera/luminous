import json
import logging as log
import queue
import socket
import struct
import threading
from pathlib import Path

import open_clip
import pyarrow as pa
import torch
from PIL import Image

import lancedb

HOST = "127.0.0.1"
PORT = 50022

# TODO: more extensions
VALID_EXT = {".jpg", ".jpeg", ".png", ".webp"}
EMBED_DIM = 512
MODEL = "MobileCLIP-B"
PRETRAINED = "datacompdr_lt"
DB_PATH = "./lancedb"
THRESHOLD = 1.70
DIM_THRESHOLD = 1.80


def recv_msg(conn: socket.socket) -> dict | None:
    try:
        header = conn.recv(4)
        if not header:
            return None
        msg_len = struct.unpack(">I", header)[0]
        chunks: list[bytes] = []
        received = 0
        while received < msg_len:
            chunk = conn.recv(min(msg_len - received, 4096))
            if not chunk:
                raise RuntimeError("Connection broken")
            chunks.append(chunk)
            received += len(chunk)
        return json.loads(b"".join(chunks))
    except (ConnectionResetError, RuntimeError):
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
        self.db = lancedb.connect(DB_PATH)
        self._tbl = None
        self._indexing = False
        self._queue: queue.Queue[tuple[dict, socket.socket] | None] = queue.Queue(
            maxsize=1
        )
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while True:
            item = self._queue.get()
            if item is None:
                break
            cmd, conn = item
            self._handle_index(cmd, conn)

    def _handle_index(self, cmd: dict, conn: socket.socket) -> None:
        folder = Path(cmd["folder"])
        paths = [p for p in folder.glob("*") if p.suffix.lower() in VALID_EXT]

        if "images" in self.db.table_names():
            tbl = self.db.open_table("images")
            indexed = {row["path"] for row in tbl.to_arrow().to_pylist()}
            paths = [p for p in paths if str(p) not in indexed]
        else:
            schema = pa.schema(
                [
                    pa.field("path", pa.utf8()),
                    pa.field("embedding", pa.list_(pa.float32(), EMBED_DIM)),
                ]
            )
            tbl = self.db.create_table("images", schema=schema)

        log.info(f"Indexing {len(paths)} new images in {folder}")
        skipped = 0
        for p in paths:
            try:
                img = self.preprocess(Image.open(p)).unsqueeze(0).to(self.device)
                with torch.no_grad():
                    vec = self.model.encode_image(img).float()
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
                skipped += 1

        self._tbl = tbl
        self._indexing = False
        log.info(f"Indexing done. {tbl.count_rows()} total, {skipped} skipped.")
        send_resp(conn, {"status": "ok", "count": tbl.count_rows()})

    def enqueue_index(self, cmd: dict, conn: socket.socket) -> bool:
        try:
            self._queue.put_nowait((cmd, conn))
            self._indexing = True
            return True
        except queue.Full:
            return False

    def is_ready(self) -> bool:
        return self._tbl is not None and not self._indexing

    def ensure_table(self) -> None:
        if self._tbl is None and "images" in self.db.table_names():
            self._tbl = self.db.open_table("images")

    def search(self, query: str, limit: int = 20) -> list[dict]:
        with torch.no_grad():
            tokens = self.tokenizer([f"a photo of {query}"]).to(self.device)
            vec = self.model.encode_text(tokens).float()
            vec /= vec.norm(dim=-1, keepdim=True)
        results = self._tbl.search(vec.squeeze().cpu().numpy()).limit(limit).to_list()
        results.sort(key=lambda r: r["_distance"])
        return [r for r in results if r["_distance"] <= DIM_THRESHOLD]

    def stop(self) -> None:
        self._queue.put(None)
        self._thread.join()


def handle_connection(conn: socket.socket, addr: tuple, worker: Worker) -> None:
    log.info(f"Host connected from {addr}")
    conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    with conn:
        while True:
            cmd = recv_msg(conn)
            if cmd is None:
                log.info("Host disconnected.")
                break
            log.debug(f"{cmd=}")

            try:
                action = cmd.get("action")
                log.debug(f"{action=}")

                if action == "ping":
                    log.debug("ping -> ok")
                    send_resp(conn, {"status": "ok"})

                elif action == "index":
                    if not worker.enqueue_index(cmd, conn):
                        log.debug("index -> busy")
                        send_resp(conn, {"status": "busy"})

                elif action == "search":
                    worker.ensure_table()
                    if not worker.is_ready():
                        send_resp(conn, {"status": "busy"})
                        log.debug("search -> not ready")
                    else:
                        results = worker.search(cmd["query"], cmd.get("limit", 20))
                        paths = [
                            r["path"] for r in results if r["_distance"] <= THRESHOLD
                        ]
                        log.debug(f"search '{cmd['query']}' -> {len(paths)} results")
                        dim_paths = [
                            r["path"]
                            for r in results
                            if r["_distance"] <= DIM_THRESHOLD
                        ]
                        log.debug(
                            f"dim search '{cmd['query']}' -> {len(dim_paths)} results"
                        )
                        send_resp(conn, {"SearchResult": {"paths": paths}})

                elif action == "shutdown":
                    log.debug("shutdown -> ok")
                    send_resp(conn, {"status": "ok"})
                    worker.stop()
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
