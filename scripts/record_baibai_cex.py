"""Снимок публичных спотовых справочников адаптеров бота: точный базовый тикер NVDAc и отдельные похожие имена."""
import hashlib
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

import requests

from record_baibai import FOLDER, write

MARKETS = {
    "binance": ("https://api.binance.com/api/v3/exchangeInfo", "symbols", "baseAsset", "symbol"),
    "bingx": ("https://open-api.bingx.com/openApi/spot/v1/common/symbols", "data.symbols", "symbol", "symbol"),
    "bitget": ("https://api.bitget.com/api/v2/spot/public/symbols", "data", "baseCoin", "symbol"),
    "bybit": ("https://api.bybit.com/v5/market/instruments-info?category=spot", "result.list", "baseCoin", "symbol"),
    "coinex": ("https://api.coinex.com/v2/spot/market", "data", "base_ccy", "market"),
    "gate": ("https://api.gateio.ws/api/v4/spot/currency_pairs", "", "base", "id"),
    "htx": ("https://api.huobi.pro/v1/common/symbols", "data", "base-currency", "symbol"),
    "kraken": ("https://api.kraken.com/0/public/AssetPairs", "result", "wsname", "altname"),
    "kucoin": ("https://api.kucoin.com/api/v2/symbols", "data", "baseCurrency", "symbol"),
    "mexc": ("https://api.mexc.com/api/v3/exchangeInfo", "symbols", "baseAsset", "symbol"),
    "okx": ("https://www.okx.com/api/v5/public/instruments?instType=SPOT", "data", "baseCcy", "instId"),
    "phemex": ("https://api.phemex.com/public/products", "data.products", "baseCurrency", "symbol"),
    "poloniex": ("https://api.poloniex.com/markets", "", "baseCurrencyName", "symbol"),
    "xt": ("https://sapi.xt.com/v4/public/symbol", "result.symbols", "baseCurrency", "symbol"),
}


def lookup(item):
    venue, (url, path, key, symbol) = item
    data = dict(venue=venue, url=url)
    try:
        response = requests.get(url, timeout=15)
        data.update(http=response.status_code, sha256=hashlib.sha256(response.content).hexdigest())
        response.raise_for_status()
        rows = response.json()
        for part in filter(None,path.split(".")):
            rows = rows[part]
        if isinstance(rows, dict):
            rows = list(rows.values())
        if venue == "phemex":
            rows = [r for r in rows if r.get("type","").lower()=="spot"]
        if not isinstance(rows,list) or not rows:
            raise ValueError("Отсутствует непустой список рынков")
        data.update(listed=len(rows), parsed=0, exact=[], related=[])
        for row in rows:
            raw = row.get(key)
            if not isinstance(raw,str) or not raw:
                continue
            data["parsed"] += 1
            base = raw.upper().split("-")[0].split("/")[0]
            if "NVDA" in base:
                data["exact" if base=="NVDAC" else "related"].append(row[symbol])
        if data["parsed"] != len(rows):
            data["incomplete"] = True
    except (requests.RequestException, ValueError, KeyError, TypeError) as error:
        data["unavailable"] = str(error)[:160]
    return data


def aliases():
    urls = {
        "htx":"https://api.huobi.pro/v2/reference/currencies?currency=nvda",
        "poloniex":"https://api.poloniex.com/currencies/NVDA?includeMultiChainCurrencies=true",
        "xt":"https://sapi.xt.com/v4/public/currencies?currency=nvda",
    }
    records = []
    for venue,url in urls.items():
        res = requests.get(url,timeout=15)
        res.raise_for_status()
        raw = res.json()
        if venue=="htx":
            rows = [{k:c.get(k) for k in ("baseChain","contractAddress","depositStatus","withdrawStatus")}
                    for r in raw["data"] if r["currency"].upper()=="NVDA" for c in r["chains"]]
        elif venue=="poloniex":
            rows = [{k:raw["NVDA"].get(k) for k in ("blockchain","walletDepositState","walletWithdrawalState","childChains")}]
        else:
            rows = [c for r in raw["result"]["currencies"] if r["currency"].upper()=="NVDA" for c in r["chainRelation"]]
        records.append(dict(venue=venue,url=url,observed=rows,sha256=hashlib.sha256(res.content).hexdigest()))
    return records


if __name__ == "__main__":
    with ThreadPoolExecutor(max_workers=5) as workers:
        rows = list(workers.map(lookup,MARKETS.items()))
    write(FOLDER/"baibai_cex_base.json",dict(observed_utc=datetime.now(timezone.utc).isoformat(),markets=rows,similar_nvda=aliases()))
