"""Сравнить прежний и новый код конструктора Dexalot на сохранённом блоке Base."""
import json
import sys
from pathlib import Path

from record_dexalot_base import Read
from verify_wallet_creation import verify


if __name__ == "__main__":
    verify(json.loads(Path(sys.argv[1]).read_bytes()), Path(sys.argv[2]),
           "dexalot-base-create-legacy.json", "dexalot-base-create.json", 8453, "router", Read())
