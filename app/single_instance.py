"""单实例 —— 同一份用户配置下只允许跑一个 VisualSpectrum。

这道程序是**常驻后台、一直往屏幕上画东西**的，跑起两份的后果是叠出两圈频谱、白烧一份 CPU。
两件机制，主次分明：

* **锁文件**（`QLockFile`，放在用户数据目录）：**权威**。保证"同一份用户配置下只有一个进程"，
  还会自己识别**陈旧锁**（上次崩溃留下的 —— 看文件里记的 PID 还在不在）。
* **本地套接字**（`QLocalServer` / `QLocalSocket`）：**锦上添花**。第二个实例可以请第一个
  "把配置页打开"（不然它只能默默退出，用户以为没反应）。

⚠ `acquire()` 返回的 `self` 要**一直攥着**到进程结束：锁与套接字都挂在它身上，回收了门就开了。
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from pathlib import Path

from PyQt6.QtCore import QLockFile
from PyQt6.QtNetwork import QLocalServer, QLocalSocket

__all__ = ["MSG_OPEN_CONFIG", "lock_path", "server_name", "SingleInstance"]

#: 第二个实例发给第一个实例的信号：把配置页打开（端口由**第一个实例**自己决定）
MSG_OPEN_CONFIG = b"open-config"

_CONNECT_MS = 300
_WRITE_MS = 200
_READ_MS = 250


def _digest() -> str:
    """用户数据目录的短哈希 —— 用它区分"不同 `VISUALSPECTRUM_HOME` 的两份"。"""
    from . import paths

    return hashlib.sha1(str(paths.config_dir()).encode("utf-8")).hexdigest()[:12]


def server_name() -> str:
    from . import APP_NAME

    return f"{APP_NAME}-{_digest()}"


def lock_path() -> Path:
    from . import paths

    return paths.config_dir() / "instance.lock"


class SingleInstance:
    """判定"本次是不是唯一实例"，并（在是的时候）持有锁与本地套接字服务端。"""

    def __init__(self, *, name: str | None = None, lock_file: Path | None = None) -> None:
        self._name = name or server_name()
        self._lock_path = lock_file or lock_path()
        self._lock: QLockFile | None = None
        self._server: QLocalServer | None = None

    @property
    def name(self) -> str:
        return self._name

    def acquire(self, on_message: Callable[[bytes], None] | None = None) -> bool:
        """`True` = 抢到了（本进程就是唯一实例，继续启动）；
        `False` = 已经有一个在跑 —— 本进程已经（尽力）请它打开配置页，应当**直接退出**。
        """
        lock = QLockFile(str(self._lock_path))
        lock.setStaleLockTime(0)                  # 长驻程序：只由"进程还在不在"决定
        try:
            self._lock_path.parent.mkdir(parents=True, exist_ok=True)
        except OSError:                           # 用户数据目录建不出来 → 别把启动也搭进去
            return True
        if not lock.tryLock():
            self._poke()
            return False
        self._lock = lock
        self._serve(on_message)
        return True

    def unlock(self) -> None:
        if self._lock is not None:
            self._lock.unlock()
            self._lock = None

    # ------------------------------------------------------------------ 内部
    def _poke(self) -> None:
        sock = QLocalSocket()
        sock.connectToServer(self._name)
        if sock.waitForConnected(_CONNECT_MS):
            sock.write(MSG_OPEN_CONFIG)
            sock.flush()
            sock.waitForBytesWritten(_WRITE_MS)
            sock.disconnectFromServer()

    def _serve(self, on_message: Callable[[bytes], None] | None) -> None:
        QLocalServer.removeServer(self._name)     # 清掉上次留下的死套接字
        server = QLocalServer()
        if not server.listen(self._name):
            return
        server.newConnection.connect(lambda: self._accept(server, on_message))
        self._server = server

    @staticmethod
    def _accept(server: QLocalServer, on_message: Callable[[bytes], None] | None) -> None:
        while server.hasPendingConnections():
            conn = server.nextPendingConnection()
            if conn is None:
                continue
            payload = b""
            if conn.waitForReadyRead(_READ_MS):
                payload = conn.readAll().data()
            if on_message is not None:
                on_message(payload)
            conn.disconnectFromServer()
