"""Read-only SSH helper, sent to the server with python3 -c."""
import os
import shutil
import stat
import sys


def emit(value):
    sys.stdout.buffer.write(str(value).encode("utf-8") + b"\0")


operation, requested = sys.argv[1:]
path = os.path.realpath(os.path.expanduser(requested))
if operation == "list":
    entries = []
    with os.scandir(path) as directory:
        for entry in directory:
            try:
                info = entry.stat(follow_symlinks=False)
                entries.append((entry.name, int(entry.is_dir()), info.st_size,
                                max(0, info.st_mtime_ns), int(entry.is_symlink())))
            except FileNotFoundError:
                continue
    emit("FVSSH1")
    emit(path)
    for entry in entries:
        for value in entry:
            emit(value)
elif operation == "read":
    # Opening a FIFO or device could otherwise hang or stream indefinitely.
    if not stat.S_ISREG(os.stat(path).st_mode):
        raise ValueError("일반 파일만 내려받을 수 있습니다")
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("파일 종류가 변경됐습니다")
        sys.stdout.buffer.write(b"FVFILE1\0" + info.st_size.to_bytes(8, "big"))
        shutil.copyfileobj(source, sys.stdout.buffer)
else:
    raise ValueError("알 수 없는 요청")
