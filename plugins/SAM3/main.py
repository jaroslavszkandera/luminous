import json
import logging as log
import queue
import socket
import struct
import threading
from multiprocessing import resource_tracker, shared_memory
from time import time

import numpy as np
import torch
from PIL import Image

import hf_login

log.getLogger("httpx").setLevel(log.WARNING)
log.getLogger("httpcore").setLevel(log.WARNING)
log.getLogger("transformers").setLevel(log.WARNING)

HOST = "127.0.0.1"
PORT = 50023
MODEL_ID = "facebook/sam3"


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


class Sam3Predictor:
    def __init__(self, device: str) -> None:
        from transformers import Sam3Model, Sam3Processor

        self.device = device
        self.processor: Sam3Processor = Sam3Processor.from_pretrained(MODEL_ID)
        self.model: Sam3Model = Sam3Model.from_pretrained(MODEL_ID).to(device)
        self.model.eval()

        self._image: np.ndarray | None = None
        self._pil: Image.Image | None = None

    def set_image(self, img_rgb: np.ndarray) -> None:
        self._image = img_rgb
        self._pil = Image.fromarray(img_rgb)

    # WARN: Does not work, documentation is different from implementation.
    def predict_point(self, x: int, y: int) -> np.ndarray:
        inputs = self.processor(
            images=self._pil,
            input_points=[[[[x, y]]]],
            input_labels=[[1]],
            return_tensors="pt",
        ).to(self.device)
        return self._run(inputs)

    def predict_box(self, x1: int, y1: int, x2: int, y2: int) -> np.ndarray:
        w = x2 - x1
        h = y2 - y1
        inputs = self.processor(
            images=self._pil,
            input_boxes=[[[x1, y1, w, h]]],
            input_boxes_labels=[[1]],
            return_tensors="pt",
        ).to(self.device)
        return self._run(inputs)

    def set_text_prompt(self, query: str) -> np.ndarray:
        if self._pil is None:
            raise RuntimeError("No image set. Call set_image() first.")
        inputs = self.processor(
            images=self._pil,
            text=query,
            return_tensors="pt",
        ).to(self.device)
        return self._run(inputs)

    def _run(self, inputs) -> np.ndarray:
        with torch.no_grad():
            outputs = self.model(**inputs)

        results = self.processor.post_process_instance_segmentation(
            outputs,
            threshold=0.5,
            mask_threshold=0.5,
            target_sizes=inputs.get("original_sizes").tolist(),
        )[0]

        masks = results.get("masks")
        print(f"n_masks={masks.shape[0]}")
        if masks is not None and len(masks) > 0:
            return masks.cpu().numpy().astype(bool)

        print("Returning no masks")
        orig_size = inputs["original_sizes"][0]
        return np.zeros((1, orig_size[0], orig_size[1]), dtype=bool)

        # FIX:
        # @property
        # def ready(self) -> bool:
        #     return self.state is not None


class Worker:
    def __init__(self, predictor: Sam3Predictor) -> None:
        self.predictor = predictor
        self._img_w = 0
        self._img_h = 0
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


def _write_mask(shm_name: str, masks: np.ndarray, img_w: int, img_h: int) -> None:
    shm = open_shm(shm_name)
    try:
        out = np.ndarray((img_h, img_w), dtype=np.uint8, buffer=shm.buf)

        if masks.ndim == 3 and masks.shape[0] > 1:
            combined_mask = np.any(masks, axis=0)
            np.copyto(out, (combined_mask * 255).astype(np.uint8))
        else:
            m = masks[0] if masks.ndim == 3 else masks
            np.copyto(out, (m * 255).astype(np.uint8))
    finally:
        shm.close()


def handle_click(cmd: dict, predictor: Sam3Predictor, img_w: int, img_h: int) -> None:
    mask = predictor.predict_point(cmd["x"], cmd["y"])
    _write_mask(cmd["shm_name"], mask, img_w, img_h)


def handle_rect_select(
    cmd: dict, predictor: Sam3Predictor, img_w: int, img_h: int
) -> None:
    mask = predictor.predict_box(cmd["x1"], cmd["y1"], cmd["x2"], cmd["y2"])
    _write_mask(cmd["shm_name"], mask, img_w, img_h)


def handle_text_prompt(
    cmd: dict, predictor: Sam3Predictor, img_w: int, img_h: int
) -> None:
    mask = predictor.set_text_prompt(cmd["text"])
    _write_mask(cmd["shm_name"], mask, img_w, img_h)


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

                # elif action in ("click", "rect_select", "text_to_mask"):
                elif action in ("rect_select", "text_to_mask"):
                    # if not worker.predictor.ready:
                    #     send_resp(conn, "busy")
                    #     log.debug(f"{action} -> no embedding yet")
                    # else:
                    # <tab>
                    if action == "rect_select":
                        handle_rect_select(
                            cmd, worker.predictor, worker.img_w, worker.img_h
                        )
                    elif action == "text_to_mask":
                        handle_text_prompt(
                            cmd, worker.predictor, worker.img_w, worker.img_h
                        )
                    # Does not work, mentioned in the handle click def...
                    # elif action == "click":
                    #     handle_click(cmd, worker.predictor, worker.img_w, worker.img_h)
                    log.debug(f"{action} -> ok")
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
        format="[SAM3]:%(asctime)s:%(levelname)s:%(message)s", level=log.DEBUG
    )
    log.info("Init...")

    hf_login.login()

    device = "cuda" if torch.cuda.is_available() else "cpu"
    log.info(f"Using {device=}")

    log.info(f"Loading SAM3 from HuggingFace ({MODEL_ID})...")
    predictor = Sam3Predictor(device)
    worker = Worker(predictor)

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind((HOST, PORT))
            srv.listen(1)
            log.info(f"SAM3 daemon listening on {HOST}:{PORT}")

            conn, addr = srv.accept()
            handle_connection(conn, addr, worker)

    except (OSError, KeyboardInterrupt) as e:
        log.error(f"Server error: {e}")
    finally:
        log.info("SAM3 daemon exiting...")


if __name__ == "__main__":
    main()
