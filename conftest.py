"""pytest 的根 conftest —— 只为把项目根目录放进 `sys.path`，好让 `import app` 生效。"""

from __future__ import annotations

import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parent
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))
