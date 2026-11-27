"""Сравнить прежний и новый конструкторы Fluid Lite на сохранённом блоке Ethereum."""
import json
import sys
from pathlib import Path

from record_base_slipstream_executor import rpc
from verify_wallet_creation import verify


if __name__ == "__main__":
    compiled = json.loads(Path(sys.argv[1]).read_bytes())
    # Источники исторической записи перечислены в record_fluid_dex_lite_wallet.py.
    endpoints = ("https://eth.drpc.org", "https://rpc.mevblocker.io", "https://ethereum-rpc.publicnode.com")
    def read(method, args):
        for url in endpoints:
            try:
                return rpc(url, method, args)
            except (ValueError, RuntimeError, OSError) as error:
                last = error
        raise RuntimeError(f"{method}: все источники недоступны") from last
    verify(compiled, Path(sys.argv[2]), "fluid-dex-lite-ethereum-previous.json",
           "fluid-dex-lite-ethereum.json", 1, "exchange", read)
