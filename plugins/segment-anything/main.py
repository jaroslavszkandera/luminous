import json
import logging as log
import os
import queue
import socket
import struct
import sys
import threading
from multiprocessing import resource_tracker, shared_memory
from time import time

import numpy as np
import torch
from segment_anything import SamPredictor, sam_model_registry

HOST = "127.0.0.1"
PORT = 50021


def open_shm(name: str) -> shared_memory.SharedMemory:
    shm = shared_memory.SharedMemory(name=name)
    resource_tracker._resource_tracker.unregister(shm._name, "shared_memory")
    return shm


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


def send_resp(conn: socket.socket, status: str, message: str | None = None) -> None:
    resp: dict = {"status": status}
    if message:
        resp["message"] = message
    payload = json.dumps(resp).encode()
    conn.sendall(struct.pack(">I", len(payload)) + payload)


class Worker:
    def __init__(self, predictor: SamPredictor) -> None:
        self.predictor = predictor
        self._img_w = 0
        self._img_h = 0
        # (cmd, conn)
        self._queue: queue.Queue[tuple[dict, socket.socket] | None] = queue.Queue(
            maxsize=1
        )
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    @property
    def img_w(self) -> int:
        return self._img_w

    @property
    def img_h(self) -> int:
        return self._img_h

    def enqueue_set_image(self, cmd: dict, conn: socket.socket) -> bool:
        try:
            self._queue.put_nowait((cmd, conn))
            return True
        except queue.Full:
            return False

    def _run(self) -> None:
        while True:
            item = self._queue.get()
            if item is None:
                break
            cmd, conn = item
            self._handle_set_image(cmd, conn)

    def _handle_set_image(self, cmd: dict, conn: socket.socket) -> None:
        log.debug("set_image: starting embedding")
        try:
            w, h = cmd["width"], cmd["height"]
            shm = open_shm(cmd["shm_name"])
            try:
                img = np.ndarray((h, w, 4), dtype=np.uint8, buffer=shm.buf)
                t = time()
                self.predictor.set_image(img[:, :, :3])
                log.info(f"Embedding ready in {time() - t:.3f}s ({w}x{h})")
                self._img_w = w
                self._img_h = h
            finally:
                shm.close()
            send_resp(conn, "ok")
        except Exception as e:
            log.error(f"set_image failed: {e}")
            send_resp(conn, "error", str(e))

    def stop(self) -> None:
        self._queue.put(None)
        self._thread.join()

    def embedding_ready(self) -> bool:
        return (
            hasattr(self.predictor, "features") and self.predictor.features is not None
        )


def handle_click(cmd: dict, predictor: SamPredictor, img_w: int, img_h: int) -> None:
    x, y = cmd["x"], cmd["y"]
    masks, _, _ = predictor.predict(
        point_coords=np.array([[x, y]]),
        point_labels=np.array([1]),
        multimask_output=False,
    )
    _write_mask(cmd["shm_name"], masks[0], img_w, img_h)


def handle_rect_select(
    cmd: dict, predictor: SamPredictor, img_w: int, img_h: int
) -> None:
    x1, y1, x2, y2 = cmd["x1"], cmd["y1"], cmd["x2"], cmd["y2"]
    masks, _, _ = predictor.predict(
        box=np.array([[x1, y1, x2, y2]]),
        multimask_output=False,
    )
    _write_mask(cmd["shm_name"], masks[0], img_w, img_h)


def _write_mask(shm_name: str, mask: np.ndarray, img_w: int, img_h: int) -> None:
    shm = open_shm(shm_name)
    try:
        out = np.ndarray((img_h, img_w), dtype=np.uint8, buffer=shm.buf)
        np.copyto(out, (mask * 255).astype(np.uint8))
    finally:
        shm.close()


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

                if action == "ping":
                    log.debug("ping -> ok")
                    send_resp(conn, "ok")

                elif action == "set_image":
                    if not worker.enqueue_set_image(cmd, conn):
                        log.debug("set_image -> busy")
                        send_resp(conn, "busy")

                elif action == "click":
                    if not worker.embedding_ready():
                        send_resp(conn, "busy")
                        log.debug("click -> no embedding yet")
                    else:
                        handle_click(cmd, worker.predictor, worker.img_w, worker.img_h)
                        log.debug("click -> ok")
                        send_resp(conn, "ok")

                elif action == "rect_select":
                    if not worker.embedding_ready():
                        send_resp(conn, "busy")
                        log.debug("rect_select -> no embedding yet")
                    else:
                        handle_rect_select(
                            cmd, worker.predictor, worker.img_w, worker.img_h
                        )
                        log.debug("rect_select -> ok")
                        send_resp(conn, "ok")

                elif action == "shutdown":
                    log.debug("shutdown -> ok")
                    send_resp(conn, "ok")
                    worker.stop()
                    return

                else:
                    log.error(f"Unknown action: {action}")
                    send_resp(conn, "error", f"Unknown action: {action}")

            except Exception as e:
                log.error(f"Processing error: {e}")
                send_resp(conn, "error", str(e))


def main() -> None:
    log.basicConfig(
        format="[SAM]:%(asctime)s:%(levelname)s:%(message)s", level=log.DEBUG
    )
    log.info("Init...")
    device = "cuda" if torch.cuda.is_available() else "cpu"
    log.info(f"Using {device=}")

    checkpoint_path = "sam_vit_b_01ec64.pth"
    if not os.path.exists(checkpoint_path):
        log.critical(
            f"SAM checkpoint not found at {os.path.abspath(checkpoint_path)}\n"
            f"Download from: https://github.com/facebookresearch/segment-anything"
        )
        sys.exit(1)

    log.info("Loading SAM model...")
    sam = sam_model_registry["vit_b"](checkpoint=checkpoint_path).to(device=device)
    worker = Worker(SamPredictor(sam))

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind((HOST, PORT))
            srv.listen(1)
            log.info(f"SAM daemon listening on {HOST}:{PORT}")

            conn, addr = srv.accept()
            handle_connection(conn, addr, worker)

    except (OSError, KeyboardInterrupt) as e:
        log.error(f"Server error: {e}")
    finally:
        log.info("SAM daemon exiting...")


if __name__ == "__main__":
    main()
