import json
import mmap
import os
import struct
import sys


def get_shm(size):
    shm_id = sys.stdin.readline().strip()
    path = shm_id if os.path.exists(shm_id) else f"/dev/shm/{shm_id.lstrip('/')}"
    f = open(path, "r+b")
    return mmap.mmap(f.fileno(), size)


def handle_decode(path):
    w, h = 500, 500
    size = w * h * 4
    print(
        json.dumps({"status": "ready", "width": w, "height": h, "required_bytes": size})
    )
    sys.stdout.flush()

    mm = get_shm(size)
    mm.write(struct.pack("BBBB", 255, 0, 0, 255) * (w * h))
    mm.close()


def handle_encode(path):
    meta = json.loads(sys.stdin.readline())
    mm = get_shm(meta["required_bytes"])
    with open(path, "wb") as f:
        f.write(mm[:])
    mm.close()


def handle_filter():
    meta = json.loads(sys.stdin.readline())
    mm = get_shm(meta["required_bytes"])
    mm.close()
    raise NotImplementedError


def main():
    cmd = sys.argv[1]
    if cmd == "decode":
        handle_decode(sys.argv[2])
    elif cmd == "encode":
        handle_encode(sys.argv[2])
    elif cmd == "filter":
        handle_filter()


if __name__ == "__main__":
    main()
